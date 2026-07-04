# B1+B2 — word-boundary wrap + nested-list indent (one document-text-rendering effort)

**Status:** CLEAN — Codex spec review ×3 (r3 CLEAN) + Fable5 ×2 (r2 READY; its r1 included
an EMPIRICAL probe of the vendored unicode-linebreak crate — the C1/I1/I2 findings rest on
compiled-and-run evidence, not reading), 2026-07-04.
**Effort:** B1 (word-boundary wrap, `Larger`) + B2 (sub-list bullet indent + hanging indent,
`Medium-small`) — **combined by user decision 2026-07-04**: document text rendering updates
in one effort, not two.
**Date:** 2026-07-04 · **Facts as of:** `9e13164` (post-H1; layout.rs = wordcartel-core,
wrap loop :260-:292; md_parse.rs `apply_block_prefix_conceal` :153-:327)

## Why

The soft-wrap is greedy per-grapheme (layout.rs:260-292): when `col + vg.width > vw` the
overflowing grapheme moves to the next row — words break mid-word at the viewport edge.
This is the backlog's highest-value rendering fix. Separately, nested list items render as
`•   sub` — `apply_block_prefix_conceal` (md_parse.rs:252-291) conceals the marker but the
leading indent SPACES survive as visible graphemes while the `"• "` glyph always paints at
column 0. The two meet at the wrap loop: fixing B2 by folding the indent into the prefix
makes wrapped nested items hang correctly for free, because continuation rows already reset
to `prefix_width` (layout.rs:281) and the renderer already paints a matching
`" ".repeat(prefix_width)` spacer (render.rs:445-446, :510-:511). **Hanging indent already
exists** — B2's job is to make `prefix_width` tell the truth for nested items.

## Decisions (user-approved 2026-07-04)

1. **One effort, both items** — document text rendering updates in a single pass.
2. **Break rules = UAX #14** (fork 1 = B, revised from whitespace-only when the user asked
   for hyphen breaking): new dependency `unicode-linebreak` in `wordcartel-core` (tiny,
   table-driven, no unsafe — compatible with `#![forbid(unsafe_code)]`; fits the existing
   `unicode-segmentation`/`unicode-width` family). Hand-rolled hyphen heuristics were
   rejected as re-deriving the standard case-by-case.
3. **Unconditional; no config key** (fork 2 = A), with ONE role-based exception:
   `BlockRole::CodeBlock` lines keep today's grapheme wrap (UAX #14 breaks at hyphens/
   spaces actively hurt code readability). No `[view]` wrap axis; if a future need appears
   the seam is the same `if` this exception uses.

## Design

### D1. Break-opportunity computation (layout.rs, before the wrap loop)

Break positions are computed on the **visible text** — concealment changes adjacency
(`"**bold**text"` renders `boldtext`; a break legal in the raw string may be illegal in the
rendered one and vice versa). Mechanically, after the `vgs: Vec<VG>` vector is built
(layout.rs:218-238): concatenate `vg.text` in order. **This is the RAW visible grapheme
text, not the rendered display string (Codex r1): `VG.text` holds `g.to_string()`
(layout.rs:232) — a tab stays `"\t"`; display expansion to TAB_WIDTH happens later when
`VisualRow.display`/segs are built (:302/:307), and tab WIDTH enters the wrap loop via
`VG.width`, not via the text.** That is correct input for break analysis — UAX #14
classifies the tab character itself (a break-after opportunity); no class depends on
visual expansion. Run `unicode_linebreak::linebreaks(&visible)` once (linear, one small
state machine — same complexity class as the segmentation pass already done per line), and
map each returned byte offset to the VG index whose text starts at that offset. **An
offset is NOT guaranteed to land on a VG start (Fable C1, empirically demonstrated):
UAX #14 and UAX #29 disagree by design — e.g. `"a \u{301}b"` yields a break offset at
the combining mark, but space+combining-mark is ONE extended grapheme cluster, so the
offset lands mid-VG. Offsets that do not coincide with a VG start are DROPPED from the
break vector (conservative: fewer opportunities, never a cluster split); the end-of-text
entry is dropped likewise.** Collect the survivors into a `Vec<usize>` of VG indices,
ascending. Mid-line `Mandatory` entries ARE reachable
(Fable I2): logical lines split on `\n` only, and U+2028/U+2029/U+000C/U+000B/U+0085
survive inside one (ropey is built default-features-off). They are treated exactly like
`Allowed` opportunities — never asserted against; only the final end-of-text entry is
ignored (dropped with the C1 rule above).

The computation is skipped entirely for `role == BlockRole::CodeBlock` (decision 3). A
width pre-check (skip when total `vg.width` ≤ capacity) is a NEW optional micro-optimization
— today's loop has no early exit (Fable M3); the plan may add it or not, it is not
load-bearing.

### D2. The wrap loop (layout.rs:260-292 reworked)

State added to the loop: `row_start_vg` (index of the first VG on the current row) and
`last_break` (the most recent legal break index ≤ current VG, maintained by advancing a
cursor over the D1 vector — O(1) amortized). The overflow branch becomes:

- **Whitespace never triggers a wrap — except in CodeBlock lines** (Fable plan review:
  the unscoped rule contradicted this spec's own "CodeBlock byte-identical" claim; in
  code, a space/tab is data and wraps greedily like any grapheme). Elsewhere: if the
  current VG's text is ASCII space or tab, it is placed at the current col even when
  `col + width > vw` — trailing whitespace hangs past the edge (standard word-processor
  behavior; a continuation row never starts with the space the user just typed). Law 3
  is amended accordingly (see Invariants).
  **Cursor rule for hung cells (Codex r1): rows paint into a clipped Rect of
  `text_width` (render.rs) and the terminal cursor is set at `text_left + col`
  (render.rs:717) — a caret logically on/after a hung whitespace cell would paint outside
  the rect. The DISPLAY column therefore clamps: the painted caret col is
  `min(col, text_width.saturating_sub(1))` (pinned at the edge, the standard editor
  behavior; saturating — the existing site's `col < tg.text_width` guard degrades
  gracefully at zero width and must stay); the
  LOGICAL mapping (`ColMap`, `screen_pos`'s returned vcol consumers) is unchanged, and
  `visual_to_source` already clamps click cols to the row end. The clamp lives at the
  render cursor-set site, not in ColMap.**
- Otherwise, on `col + vg.width > vw && col > prefix_width`:
  - if a legal break exists with `row_start_vg < break ≤ current index`: the row ends at
    the break — VGs from the break to the current index (exclusive) are RE-PLACED onto the
    new row starting at `prefix_width` (their already-pushed `Placed` entries are moved:
    row += 1, cols recomputed from `prefix_width`). **Bookkeeping (Codex r1): the broken
    row's `row_end_col` entry is pushed AT THE BREAK — its value is the col after the last
    VG that stays on the row (including hanging whitespace), NOT the col the loop had
    reached; `rows` derives from the final row counter as today (:293). `VisualRow`s,
    `display`, `segs`, and `src_span` are built from `placed` AFTER the loop (:296-…), so
    re-placement needs no display/seg repair — only `placed` rows/cols and `row_end_col`.**
    The re-placement is bounded by the row width (≤ vw cells), preserving the hot path's
    O(visible) class — each VG moves at most once per row boundary.
  - if no legal break exists on the row (one unbroken token wider than the row): fall back
    to the existing grapheme break (the current VG opens the new row). The existing
    single-grapheme guard (`col > prefix_width`) is unchanged — a grapheme wider than
    `vw - prefix_width` still places alone on its row.
- **The overflow decision REPEATS until the current VG fits (amended 2026-07-04, user-
  ratified, from a probe-confirmed Fable plan-review Critical):** a tail re-placement can
  leave the current VG still over-wide — e.g. a zero-width head makes the "freed" columns
  zero, or an em-dash head with a no-break-before tail (CL-class `。`) — and pushing it
  unchecked violates Law 3 on strategy-generable input. Each repeat either advances the
  break point strictly (b increases) or falls back at the row start (where the single-
  grapheme guard applies), so the loop terminates. W1 and Law 3 hold as stated under the
  repeat.
- CodeBlock lines never consult breaks (D1 skipped) and never hang whitespace — behavior
  byte-identical to today.

Zero-width VGs keep today's handling (placed at current col, no overflow test,
layout.rs:266-273). The `desired_col`/`snap_to_stop`/`enter_from_*` machinery is untouched
— it operates on the resulting `Placed` geometry.

### D3. B2 — nested-list indent conceal (md_parse.rs:252-291, ListItem arm ONLY)

Today: `start` = count of leading SPACE bytes ONLY — the scan at md_parse.rs:253 is not
tab-aware, so a tab-indented item is not recognized as indented at all (Codex r1). New:
**the `start` scan itself extends to spaces AND tabs**, and — CONDITIONAL ON A MARKER
MATCH (Fable I4): continuation lines of a multi-line item carry `BlockRole::ListItem`
without a marker (block_tree.rs:221 spans the whole item), and today's no-marker path
returns `None` concealing nothing — that path stays byte-identical, else a continuation
line's indent would vanish with no glyph. Only inside the two marker branches are
positions `0..start` ALSO concealed, with the prefix glyph becoming **indent + marker**:

- unordered: `format!("{}• ", indent_str)` where `indent_str` reproduces the leading
  whitespace's display width — spaces copied as-is; a leading TAB contributes
  `TAB_WIDTH = 4` spaces (matching layout.rs's tab expansion, :192-:198, so the glyph
  width equals what the raw indent would have occupied).
- ordered: `format!("{}{}. ", indent_str, ordinal)` — identical treatment
  (md_parse.rs:275-289).

`prefix_width` (layout.rs:245-258) derives from the glyph string unchanged — it now equals
indent + marker width, so: the bullet paints at its indent level (`  • sub`), text follows
at the right column, and wrapped continuation rows hang under the item's TEXT (the
existing `col = prefix_width` reset + render spacer do this with zero render changes).
Generic for any nesting depth. **Blockquotes are OUT of scope** (their leading spaces stay
visible — md_parse.rs:239-250 untouched); headings/code/thematic breaks untouched.

### D4. Invariant amendments (layout.rs proptest laws :751-:1082)

- **Law 3 (softwrap fidelity, :838-:882)** amends its width bound to a COMPOSABLE form
  (Fable I3 — the two exemptions fire together on generated input, e.g. a width-2 grapheme
  at vw=1 followed by a hanging space): **row width MINUS trailing-whitespace width ≤ vw,
  unless the row's non-whitespace content is a single grapheme wider than the available
  width (which may additionally carry hanging trailing whitespace).**
- **Law 4 (active identity, :890-:909)** unchanged and still binding: every visible byte
  is placed exactly once, no gaps — the D2 re-placement must preserve total coverage
  (hanging whitespace is placed, never dropped).
- **Law 5 (desired-col round-trip, :918-:941)** unchanged.
- **New law W1 (no needless mid-word break), stated over the MAPPED VG-index vector
  (Fable M1 — a raw-text statement becomes unsatisfiable once C1 drops offsets):** for
  every non-CodeBlock row boundary whose first VG index is `j`, either `j` is a mapped
  break opportunity, or no mapped opportunity `k` satisfies `row_start_vg < k ≤ j`
  (fallback), or the boundary is the logical line start.
- **New law W2 (B2 alignment), scoped to `is_active = false`** (the active line is raw
  with `prefix_glyph = None` — md_parse.rs:13-19; Fable M5): for an inactive list-item
  line with leading indent, the glyph width equals indent display width + marker width,
  and `Placed` cols on every row start at `prefix_width`.

### D5. Consumers — verified unaffected (evidence from the 2026-07-04 code map)

`ColMap`'s contract (`source_to_visual`/`visual_to_source`/`row_end_col`/`snap_to_stop`)
is geometry-agnostic; consumers verified to handle arbitrary row-break positions:
`screen_pos` (nav.rs:83-124), `ensure_visible` (:401-483), `offset_at_cell` (:909-937),
`move_home/end/up/down/left/right`, `last_fully_visible_line` (:792-816), scrollbar and
selection painting. `typewriter_rows_of_line` (nav.rs:500-522) is a HEURISTIC (typewriter scroll anchoring
only) and its status is stated honestly (Codex r1): its early exit fires on BYTE length
`content_len + prefix_width <= text_width`, with `prefix_width` read from the cache and
approximated as 0 for uncached lines. (a) For space-indented B2 items the LAYOUT compensation is exact — indent
bytes leave the visible text as the same width enters the glyph (`"  - x"`: 4 bytes
concealed, glyph `"  • "` width 4) — while the BYTE heuristic itself becomes strictly
more conservative for cached items (raw `content_len` still counts the concealed bytes
AND the cached `prefix_width` grows): it fires less often, never wrongly (Codex r2). (b) The byte-length test is ALREADY unsound for tabs TODAY (a tab is 1 byte but
4 display cells — pre-existing, not a B1/B2 regression); tab-indented items inherit that
known limitation. (c) The uncached prefix≈0 approximation makes the exit fire more
often; a wrong exit mis-anchors typewriter scroll by a row — cosmetic, self-correcting
on the next cached frame. Word wrap itself never adds rows to content whose VISIBLE
width fits one row, so the exit's soundness class is unchanged. `visual_to_source`'s
end-of-row clamping already models short rows. The render row loop (render.rs:381-612)
consumes `VisualRow`/`prefix_width` unchanged. Folds, focus mode, centered measure, wrap
guide: orthogonal (the guide remains cosmetic at `wrap_column`).

### D6. Hot path

Per visible line, layout gains one linear `linebreaks()` pass + one break-cursor advance
inside the loop + bounded re-placement at row boundaries — O(visible line) total,
unchanged class. The `LayoutKey` cache gate (derive.rs:183-273) is untouched: layouts
recompute only when the key changes, exactly as today. Allocation growth per laid-out line: the
concatenated visible `String` (linebreaks needs a contiguous &str — Fable M2) plus the
break-index `Vec<usize>` — both O(visible), same order as `vgs` itself; acceptable.

## Testing

**Updated pins (all enumerated from the code map; update expectations, never weaken the
property being tested):**
- layout.rs: `active_line_identity_and_wrap` (:513 — `"abcdef"` @ 4: NO break opportunity
  → rows unchanged `["abcd","ef"]`; assert that explicitly), `prefix_reduces_wrap_capacity`
  (:663 — `"- aaaa bbbb"` @ 6 now breaks at the space: rows become `aaaa` /
  `bbbb`-at-col-2, with the space hanging on row 0).
- render.rs `wrapped_list_item_continuation_row_aligns_text_and_caret` (:1909): needs
  NO expectation change (Fable M6 hand-walk — at vw 12 the trailing space fits at col 11
  and the break lands at the current VG, reproducing today's rows byte-identically);
  keep it as a pin that word wrap preserves this geometry.
- nav.rs `typewriter_rows_prefix_aware` (:1487, `"- \taaaa"` @ 8 — MISSING from the
  original enumeration, Fable M6): assertions survive (2 rows before and after; the break
  follows the tab) but its doc-comment column arithmetic (:1484-1485)
  goes stale — update the comment.
- nav.rs `screen_pos_wrapped_line_second_visual_row` (:1097) and
  `caret_in_tall_wrapped_line_stays_visible` (:1112): no break opportunities in their
  corpora (`abcdef`, `aaaa…`) — must pass UNCHANGED (they now pin the fallback).
- derive.rs `long_line_wraps_at_small_width` (:417), `rebuild_fills_editing_rows…` (:561):
  corpora use unbroken runs — verify unchanged or adjust corpus deliberately.
- md_parse.rs list tests (:461, :491, :522, :530): unchanged for unindented items; NEW
  cases for `"  - sub"` (visible = `sub`, glyph = `"  • "`), tab-indented, nested
  ordered (`"   2. x"` → glyph `"   2. "`), and the MARKER-LESS ListItem continuation
  line (`"  second"` under `"- first"` — indent stays VISIBLE, glyph None; Fable I4).
- tests/block_roles_integration.rs (:9): unchanged (unindented, no wrap at width 80).

**New pins:**
- Word-break unit cases: break after space; after hyphen in `self-referential`; after
  ` — ` (em-dash prose); NO break in `non\u{a0}breaking` (NBSP). **The `-flag` case pins
  the 15.0 behavior: unicode-linebreak 0.1.5 implements Unicode 15.0.0, which ALLOWS a
  break after a word-initial hyphen (LB20a forbidding it arrived in 15.1) — so `-flag`
  MAY wrap after the `-`; accepted as a known wart of the pinned crate (Fable I1,
  empirically probed), recorded in Non-goals;** CJK `中文混排English` mixed-script breaks;
  fallback on `aaaaaaaa` and a long URL; trailing-space hang (row width may exceed vw by
  the space; the space remains placed — law 4).
- CodeBlock exemption: a fenced code line with spaces/hyphens wraps per-grapheme,
  byte-identical rows to today.
- B2: nested bullet paints at its indent column (render-level, via TestBackend); wrapped
  NESTED item's continuation row hangs under the item text (the B1×B2 composition — the
  effort's headline case); caret/click round-trip on that same wrapped nested item
  (offset_at_cell ↔ screen_pos).
- Proptest laws W1/W2 (D4) join the existing law suite over the token strategy, which
  gains a BARE combining-mark token (`"\u{301}"` — Fable C1: the existing alphabet
  always attaches marks to a base, so the mid-VG-offset landmine was ungenerable;
  layout.rs:715-736) alongside the existing é/中/🙂/ZWJ/ZWSP coverage.
- e2e Harness journey: type a paragraph past the viewport edge → no mid-word break on
  screen; navigate End/Home/up/down across the wrap; edit a nested list item until it
  wraps → bullet column + hanging indent visually pinned via `screen_contains` rows.

**Gates:** the standard set — suite green, workspace clippy deny, warning-free; smoke
quoted verbatim pre-merge (advisory). `cargo fuzz` targets and the F2 oracle are
unaffected (block_tree/incremental parsing untouched).

## Non-goals (explicit)

- No config axis for wrap; no `[view]` changes.
- No blockquote indent conceal; no task-list support; no soft-hyphen (U+00AD)
  render-time hyphenation (UAX #14 treats it as an opportunity — accepted as-is; no
  visible hyphen is synthesized at the break).
- No render.rs split (E3), no ColMap API changes, no nav.rs behavior changes beyond what
  the new geometry implies.
- Known wart, accepted: unicode-linebreak 0.1.5 = Unicode 15.0.0 (pre-LB20a), so a
  word-initial hyphen (`-flag`, `--long-flag`) may wrap after the `-`; upstream is
  maintenance-only — revisit only if it bites (Fable I1).
- Selection/search highlights on hung trailing-whitespace cells are clipped by the row
  Rect (render.rs:607-608) — invisible past the edge, accepted (Fable M7).
- B2 bullet columns mirror SOURCE indent: CommonMark treats `"- x"` and `"   - y"`
  (≤3 spaces) as the same list level, but they paint at different columns — deliberate
  source-faithfulness (Fable M7).
- Justification/alignment, hyphenation dictionaries: out of scope permanently unless the
  user asks.

## Ship-time bookkeeping

Backlog: B1 and B2 → SHIPPED (one entry note: combined effort); note the CodeBlock
exception and the UAX #14 dependency in the B1 entry. Memory: working order advances
(next = C4 per the recorded order). Ledger: standard per-task lines.
