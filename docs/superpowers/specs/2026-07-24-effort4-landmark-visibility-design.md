# Effort ④ — landmark visibility: design spec

**Date:** 2026-07-24. **Status:** draft for Codex spec gate.
**Items:** B12 (lone block-begin renders nothing) + B13 (block markers — styled boundary
cells, modern B-lite) + the 1B scope extension (char-mark / bookmark visibility), with the
C2 mark-removal rider. H25 confirmed OUT of scope (all new styling is additive; §3.6).
**Branch:** `effort-4-landmark-visibility` off main @ `6067ad5`. No source edits before
plan execution.
**Grounding inputs:** `scratchpad/group4/` (three explore sweeps + `fable-grounding-brief.md`
+ `group4-forks.md` Parts 1–4), `docs/design/cursor-system-concept-review.md` §3.2; every
claim below re-verified against the working tree at `6067ad5`.

All anchors are SYMBOL names (file + symbol), never line numbers, except where a specific
expression is quoted. Claims that could not be verified by reading are collected in §10.

**Framing intent (human, 2026-07-24):** marked blocks and marks are **navigation landmarks
and reminders you can SEE** — not merely block-selection endpoints. Visibility is the
enabling feature; every design choice below serves "spot it, know it, jump to it."

---

## 1. Summary and locked decisions

Human-locked decisions (2026-07-24, forks + 1B sub-forks resolved from
`scratchpad/group4/group4-forks.md`):

1. **Scope = 1B.** ④ makes ALL landmarks visible: the marked block (interior + boundaries
   + pending anchor) AND the char marks / numbered bookmarks 0-9 — all riding one new
   feature module, `wordcartel/src/block_paint.rs`.
2. **Block styling (B13).** The completed-block INTERIOR becomes a quiet tint: each
   theme's `marked_block` face keeps its Highlight-contract `bg` and DROPS the
   `reverse+bold+underline` stack (no-color, which has no bg, becomes `reverse+dim`).
   A single new `SemanticElement::MarkedBlockBoundary` carries the strong cues at the
   begin and end cells. **End boundary = the LAST INTERIOR glyph (the glyph containing
   `b.end - 1`), not the glyph at `b.end`.** This is a **deliberate visible change to the
   shipped block look** (today a block renders as a full reversed-bold-underlined span).
3. **Pending ^KB (B12) = 2C.** A boundary cell at the anchor glyph, reusing the
   begin-boundary face (no separate pending element), plus a `· BLK…` status segment for
   the mid-mark mode. EOL/empty-line anchors (no glyph to style) get a painted one-cell
   trailing marker per the fold-marker span-append precedent (§3.3).
4. **Off-screen hint = 4B, block-only.** The status `· BLK` segment gains a direction:
   `· BLK↑` / `· BLK↓` when the block is entirely above/below the visible window, plain
   `· BLK` when any part is in view. NO per-mark directional hints.
5. **Landmark identity (sub-fork A).** Presence cells — a new `SemanticElement::
   LandmarkGlyph` styled onto the EXISTING glyph at each mark position — plus a caret-line
   status segment `· MK <ids>` (e.g. `· MK 3,a`) listing the marks on the caret's line.
   NO injected glyphs, zero ColMap/layout impact (B-lite). Element name `LandmarkGlyph`
   is the human's pick (covers both numbered bookmarks and named char marks).
6. **Mark removal (sub-fork C = C2, folded in).** Two new commands — `clear_mark`
   (interactive slot pick via the existing `pending_mark` intercept) and `clear_marks`
   (clear all in buffer). These are the ONLY new commands in ④.
7. **Visibility is ALWAYS-ON.** No user-settable toggle/option.
8. **Precedence + exclusive classification (sub-fork D).** At a shared cell:
   block boundary > pending > landmark > interior. `block_paint` classifies each cell as
   EXACTLY ONE of these and patches ONE face — landmark faces are never stacked on each
   other (sidesteps add-only modifier accumulation; the mono cue-space is near-full,
   §3.6). Selection/Search/ProseLens/Diag still patch above, unchanged.
9. **Undo drift: ACCEPT + DOCUMENT.** Marks survive undo un-remapped (stale-but-clamped —
   `Buffer::apply` remaps them per edit via `map_pos`, but `Editor::undo`/`redo` do not
   touch them; jumps clamp via `nav::clamp_snap`). Painting makes this pre-existing
   behavior visible for the first time. Do NOT clear marks on undo (hostile to the
   reminder intent); a true remap needs a core history-API change (out of scope). §4.2.
10. **Cue allocation** (confirmed against every theme incl. terminal-plain/no-color,
    §3.6): interior `reverse+dim` (mono) / bg-tint (RGB); boundary
    `reverse+bold+underline`; landmark `reverse+italic+underline`. All additive — **H25
    stays OUT of ④.**

Command-surface contract: **ENGAGES** (C2's two commands) — conformance in §5.
Anti-regrowth: render.rs (production 899 against its 900 `module_budgets` cap) must
**net-shrink**; accounting in §6.

---

## 2. Current behavior (grounded, symbol-anchored)

### 2.1 The block model

`wordcartel/src/editor.rs`: `pub struct MarkedBlock { pub start: usize, pub end: usize,
pub hidden: bool }` — half-open `[start, end)` byte range; `Buffer.marked_block:
Option<MarkedBlock>` (ONE per buffer) and `Buffer.pending_block_begin: Option<usize>`
(the ^KB anchor). In `Buffer::apply`, `start` remaps via `map_pos`, `end` and the pending
anchor via `map_pos_before` (boundary inserts stay outside), and a collapsed block
(`start >= end`) clears. `Editor::undo`/`redo` clear both (`// 9A: undo/redo bypass
apply's mapping → clear the block`). `session_restore.rs::load_block_from_entry`
restores a persisted block (always `hidden: false`); `persist_session` records it.

### 2.2 The mark/bookmark model

`Buffer.marks: BTreeMap<char, usize>` is the single store. Numbered bookmarks 0-9 ARE
chars `'0'..='9'` in this map: the `register_bookmarks!` macro in
`registry.rs::register_builtins` generates `set_bookmark_N`/`jump_bookmark_N` as wrappers
over `marks::set_char_mark`/`marks::jump_char_mark` (shared-slot pinned by
`bookmark_shares_slot_with_interactive_char_mark`). The interactive door
(`marks::set_mark`/`jump_to_mark`, "Set Mark…"/"Jump to Mark…") captures the next key as
the name via the `pending_mark` intercept (`marks::intercept`; `MarkPending { Set, Jump }`
in editor.rs; Esc/non-char cancels). Capacity unbounded (any `char`). **There is no
clear/delete/list affordance anywhere** — a mark can only be overwritten. In
`Buffer::apply`, all marks and the jump ring remap via `map_pos`; undo/redo leave them
untouched (stale-but-clamped, §4.2). `persist_session` writes marks as string-keyed
entries (`state.rs::StateEntry.marks: BTreeMap<String, usize>`); `load_marks_from_entry`
restores clamped. `jump_char_mark` records the jump ring, clamps, unfolds
(`place_caret_visible(…, CaretPlace::UnfoldTo)`), ensures visible. All 24 mark commands
register with `MenuCategory::None` (palette-only).

### 2.3 Current rendering

`render.rs::gather_row_ctx` snapshots `marked_block` and computes
`use_placed = !hl_window.is_empty() || diag_active || has_sel || has_block ||
prose_lens_active` — **a lone `pending_block_begin` does NOT force the placed path**, and
`RowCtx` has no pending field. In `render.rs::row_spans_placed`, a visible block patches
per-glyph:

```rust
if let Some(b) = ctx.marked_block {
    if !b.hidden && overlaps(g_from, g_to, b.start, b.end) {
        let mb_face = editor.theme.face(SE::MarkedBlock);
        style = style.patch(crate::compose::face_to_ratatui(&mb_face, editor.depth));
    }
}
```

Composition order: base ladder → MarkedBlock → Selection → Search → ProseLens → Diag.
Fold-safe for free (hidden lines never reach `map.placed`); ventilate-safe (`line_off`
from `ventilate::origin_of`). `pending_block_begin` has ZERO render-side uses (verified
by grep across the shell: only editor.rs remap/clear and blocks_marked.rs set/consume).
`Buffer.marks` likewise has ZERO render-side uses. The status line
(`render_status.rs::status_left_text`) appends `" · BLK"` / `" · BLK·hidden"`; nothing
for pending or marks. The fold marker in `render.rs::paint_rows` already appends painted
non-document spans (`"▸ "` prefix, `"  … {n} lines"` suffix) — the precedent for ④'s
trailing marker cell.

### 2.4 Compose and theme machinery

`compose.rs::face_to_ratatui` is add-only (`add_modifier`; colors set, never cleared;
at `Depth::None` colors are stripped entirely — "cue mode (None) carries NO color").
`compose.rs::merge` is `.or()`-layering. `ratatui::Style::patch` is add-only. Therefore
any face whose cue is purely additive composes safely — the H25 subtraction gap is not
touched (§3.6).

`wordcartel-core/src/theme.rs`: `SemanticElement` (exhaustive across `Theme::face`,
`face_mut`, `element_from_key`, the `ALL_ELEMENTS` totality array [35 entries], and the
test-side `face_requirement` classifier). `MarkedBlock`'s face in every constructor today
is `bg + reverse + bold + underline`; the seven face-literal constructor sites are
`default()` (shared by `terminal-plain`), `tokyo_night()`, `terminal_ansi()`,
`blue_jeans(name, r)` (×3 themes), `from_base16(name, p)` (×10 themes), `no_color()`,
and `phosphor(name, hue)` (×5 themes) — 22 builtins total
(`builtin_names_registry_total`). Contract tests that bind ④:
`every_rgb_builtin_satisfies_the_completeness_contract` (MarkedBlock is class
`Highlight`: bg OR reverse), `marked_block_mono_modifier_is_distinct` (currently pins
interior = reverse+bold+underline — **④ rewrites this test**, §3.6),
`prose_lens_bg_distinct_from_marked_block_in_color_themes` (lens bg ≠ block bg — ④ keeps
every interior bg unchanged, so it stays green), `face_is_total_and_heading_clamps`.

### 2.5 Budgets

`wordcartel/tests/module_budgets.rs::production_lines` counts lines before the file's
`mod tests`; `render_rs_stays_bounded` caps `src/render.rs` at 900. Measured at
`6067ad5`: **899**. `app.rs` is capped at 1000 (not touched by ④; the C2 rows land in
`registry.rs`, which has no hub budget). `clippy::too_many_lines` (threshold 100) applies
to every new function.

---

## 3. Design

### 3.1 The seam: `wordcartel/src/block_paint.rs` (new feature module)

One module owns ALL landmark visibility: gather, classification, per-glyph patching, the
trailing marker, and the status helpers. render.rs keeps only thin call sites and
**net-shrinks** (§6).

```rust
//! Landmark visibility (Effort ④): marked-block interior/boundary/pending paint +
//! char-mark/bookmark presence cells + the landmark status segments. B-lite: styles
//! EXISTING cells only — no injected glyphs, no ColMap/layout impact.

use ratatui::style::Style as RStyle;
use ratatui::text::Span;
use wordcartel_core::theme::{Depth, SemanticElement as SE, Theme};

/// Per-frame landmark snapshot (gathered once in `gather_row_ctx`, carried in `RowCtx`).
pub(crate) struct BlockPaint {
    /// The completed block, already filtered to visible (`!hidden`).
    block: Option<crate::editor::MarkedBlock>,
    /// The ^KB anchor awaiting ^KK.
    pending: Option<usize>,
    /// Mark positions (values of `Buffer.marks`), sorted ascending, deduplicated.
    landmarks: Vec<usize>,
}
```

- `pub(crate) fn gather(editor: &Editor) -> BlockPaint` — reads the active buffer:
  `block = marked_block.filter(|b| !b.hidden)`, `pending = pending_block_begin`,
  `landmarks = marks.values().copied().collect()` then `sort_unstable` + `dedup`.
  O(#marks log #marks) once per frame; #marks is small (10 digits + a handful of letters
  in practice).
- `pub(crate) fn wants_placed(&self) -> bool` — `self.block.is_some() ||
  self.pending.is_some() || !self.landmarks.is_empty()`. Replaces `has_block` in the
  `use_placed` disjunction and FIXES the B12 gate gap (§2.3). A buffer with only marks
  now takes the placed path — deliberate; the placed builder is O(visible), the same cost
  class the selection/search/lens features already accept.
- **Exclusive classification** (locked decision 8):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CellKind { Boundary, Pending, Landmark, Interior }
```

  `fn classify(&self, g_from: usize, g_to: usize) -> Option<CellKind>` — first match
  wins, using `render.rs::overlaps` (re-exported or duplicated as a private one-liner;
  see §3.7):
  1. `Boundary` — block present AND (glyph overlaps `[b.start, b.start+1)` OR overlaps
     `[b.end - 1, b.end)`). (`b.start < b.end` is a model invariant — `set_block`
     rejects empty and `Buffer::apply` clears collapsed blocks — so `b.end - 1` cannot
     underflow into a bogus range; it is `>= b.start`.)
  2. `Pending` — pending anchor `p` present AND glyph overlaps `[p, p+1)`.
  3. `Landmark` — any landmark position `m` overlaps `[m, m+1)` with the glyph. The
     glyph's span is checked against the sorted vec via
     `partition_point`/short linear scan (positions are bytes; glyph spans are short).
  4. `Interior` — block present AND glyph overlaps `[b.start, b.end)`.
- `pub(crate) fn patch_glyph(&self, style: RStyle, g_from: usize, g_to: usize,
  theme: &Theme, depth: Depth) -> RStyle` — maps `classify` to ONE face and patches:
  `Boundary`/`Pending` → `SE::MarkedBlockBoundary`, `Landmark` → `SE::LandmarkGlyph`,
  `Interior` → `SE::MarkedBlock`, `None` → style unchanged. This single call REPLACES the
  inline arm quoted in §2.3.
- `pub(crate) fn trailing_marker(&self, editor: &Editor, l: usize) -> Option<Span<'static>>`
  — called only for an entry's LAST visual row; computes the logical content end
  internally (§3.3) and returns at most one styled cell.
- Status helpers (called from `render_status.rs`, §3.4): `pub(crate) fn
  blk_direction(editor: &Editor, b: MarkedBlock) -> &'static str` and
  `pub(crate) fn marks_on_caret_line(editor: &Editor) -> Option<String>`.

`RowCtx` change: the `marked_block: Option<crate::editor::MarkedBlock>` field is REPLACED
by `block_paint: crate::block_paint::BlockPaint`; `gather_row_ctx`'s three-line
block snapshot (`marked_block`/`block_hidden`/`has_block`) collapses to
`let block_paint = crate::block_paint::gather(editor);` with
`block_paint.wants_placed()` in the `use_placed` expression.

### 3.2 Cell classification semantics (what the user sees)

- **Completed block** `[start, end)`: the glyph containing `start` and the glyph
  containing `end - 1` render with the boundary face; glyphs strictly between render with
  the quiet interior tint. A one-glyph block (`end == start + 1`) is a single boundary
  cell (classification rule 1 fires; no interior remains — correct degenerate form).
- **Pending anchor**: exactly one boundary-faced cell at the anchor glyph. On ^KK the
  anchor cell becomes the completed block's begin boundary — visual continuity for free.
- **Marks**: one landmark-faced cell per mark position, on the existing glyph. Marks at
  the same byte paint once (dedup). A mark inside a folded region or on a concealed
  markup byte (LivePreview hides e.g. `**` markers — such source bytes have no placed
  glyph) stays invisible until revealed — consistent with fold behavior everywhere else;
  jumps unfold (`CaretPlace::UnfoldTo`).
- **Fold/ventilate safety** is inherited: classification runs only over `map.placed`
  glyphs with absolute byte spans (`line_off` from `ventilate::origin_of`), exactly like
  the current block arm (§2.3).
- Selection/Search/ProseLens/Diag compose ABOVE landmark faces, unchanged in both order
  and code. (In reverse-only-Selection themes a selected boundary cell is visually
  identical to an unselected one — REVERSED is idempotent under `add_modifier`. Accepted:
  same pre-existing class as lens-over-block composition, and the hardware caret (C1
  DECSCUSR) marks the selection head independently.)

### 3.3 The trailing marker (EOL / empty-line anchors)

Marker bytes with no glyph: the pending anchor or `b.start` at end-of-line or on an empty
line, `b.end - 1` on a line's newline byte (a block ending at a line boundary), a mark
set at EOL (common — "mark here" at a line end).

**Codex spec-gate round 1 fold.** The first draft's predicate (`m == row_hi ∧ last_row`,
with `row_hi = line_off + vr.src_span.end`) was UNSOUND for concealed markup:
`VisualRow::src_span` is the span of the row's *visible* content only
(`wordcartel-core/src/layout.rs::VisualRow` — "Source byte range covered by the
*visible* content of this row"; concealed runs are skipped by the `if !run.visible {
continue; }` gather in `layout()`, and `src_span` is rebuilt from placed visible glyphs).
The source itself documents that the end-of-row visible byte can sit BEFORE a concealed
trailing marker (`layout.rs::move_end` doc: "`**a**` at width 1: visible cell 'a' ends
at byte 3, which is a `*`"). Under the old predicate a mark at such a byte would have
painted a misleading trailing cell — the opposite of the stated intent (concealed-byte
marks paint nothing until revealed). Corrected rule:

> On an entry's LAST visual row only, compute the entry's final logical line
> `end_line = ventilate::resolve(&view, buf, l).map(|r| r.last_line).unwrap_or(l)`
> (the exact upper-bound idiom `gather_row_ctx`'s search window already uses) and its
> **logical content end**
>
> ```rust
> fn logical_content_end(buf: &TextBuffer, line: usize) -> usize {
>     let start = derive::line_start(buf, line);
>     let next  = derive::line_start(buf, line + 1);   // clamps to buf.len() past the last line
>     if next > start && buf.byte(next - 1) == b'\n' { next - 1 } else { next }
> }
> ```
>
> **(Codex round 2 fold)** The newline test is a BYTE-level read, never a str slice:
> `TextBuffer::slice` asserts char boundaries, so `slice(next - 1..next)` would PANIC
> when the final line ends in a multibyte char (`"é"`: `next == len == 2`, byte 1 is the
> continuation byte of U+00E9). ④ adds the missing one-line core accessor:
>
> ```rust
> /// Returns the raw byte at offset `i` (`0 <= i < self.len()`). Byte-level accessor
> /// for single-byte sentinels (`\n`) — safe at ANY offset including mid-codepoint,
> /// unlike `slice`, which asserts char boundaries.
> ///
> /// # Panics
> /// Panics if `i >= self.len()` (ropey bounds check).
> pub fn byte(&self, i: BytePos) -> u8 { self.rope.byte(i) }
> ```
>
> in `wordcartel-core/src/buffer.rs` (delegating to `ropey::Rope::byte`, present in the
> pinned ropey 1.6.1). In-bounds by construction here: `next > start` implies
> `start <= next - 1 < next <= buf.len()`. The comparison is unambiguous: `0x0A` is an
> ASCII byte and UTF-8 continuation bytes are `0x80..=0xBF`, so a newline byte can never
> be the interior of a multibyte codepoint.
>
> For each marker byte `m` (pending, `b.start`, `b.end - 1`, each landmark), append ONE
> styled cell iff `m == logical_content_end(buf, end_line)`. At most one cell per row;
> priority Boundary > Pending > Landmark (locked decision 8).

Why this is sound: the logical content end is the newline byte's own offset (or
`buf.len()` on a final line without one) — a position NEVER covered by a placed glyph
(glyphs are visible line content; the newline/EOF position is not a glyph), so the
trailing cell can neither duplicate nor contradict a glyph-styled marker. Any marker
byte NOT at the logical content end either has a placed glyph (styled in place by
`classify`) or is concealed/folded content — which paints nothing until revealed, the
§3.2 rule, now honored by construction. A soft-wrap boundary byte belongs to the next
row's `row_lo` and is glyph-styled there; the last-row guard plus the `m == cend` test
make a duplicate impossible. Interior EOL bytes of a multi-line ventilated entry are not
`end_line`'s content end and get no cell (rare; discoverable via status + jumps).

Worked test vectors (pinned in `block_paint` unit tests, §7):
- `"abc\n"`, mark at byte 3 (true EOL): `next = 4`, byte 3 is `\n` → `cend = 3`;
  `m == cend` → the trailing `·` FIRES on line 0's last row.
- `"**a**\n"` (LivePreview conceals `**` runs; the only placed glyph is `a` at 2..3),
  mark at byte 3 (a concealed trailing `*`): `cend = 5` (the newline); `3 != 5` → NO
  trailing cell, and no placed glyph covers byte 3 → the mark paints NOTHING until
  revealed (SourcePlain / concealment off) — exactly the stated intent.
- `"**a**\n"`, mark at byte 5 (true EOL): `m == cend = 5` → FIRES, appended after the
  visible `a`.
- Empty line (`"\n"` at line start `p`): `cend = p`; a marker at `p` fires on the empty
  row. Final line without a trailing newline (`"abc"`): `cend = len = 3`; a marker at 3
  fires. Block `[0, 4)` over `"abc\n"`: end marker byte `3 == cend` → trailing `]`.
- **Multibyte EOF (round-2 vector):** `"é"` (2 bytes, no trailing newline), mark at EOF
  (byte 2): `start = 0`, `next = buf.len() = 2` (clamped); `buf.byte(1)` is `0xA9` (the
  U+00E9 continuation byte) `!= b'\n'` → `cend = 2`; the marker at 2 FIRES — and the
  helper does NOT panic (the old `slice(1..2)` would have tripped the char-boundary
  assert).

Primitives (all verified in source): `lines::line_start` (re-exported as
`derive::line_start`) clamps out-of-range lines to `buf.len()`, so `line + 1` is safe on
the last line; `TextBuffer::byte_to_line` exists; `TextBuffer::byte` is ④'s one-line
core addition over `ropey::Rope::byte` (ropey 1.6.1, verified present);
`ventilate::resolve` is `pub` and returns `Resolved { last_line, .. }`. Audit: this
helper was the ONLY spot in ④ touching buffer content at an arbitrary offset — every
other new read is integer byte-range math (`overlaps` on glyph spans, `byte_to_line` /
`line_start` lookups); no other `slice` call at a possibly-non-boundary offset exists in
the design.

Marker glyphs: `[` for Boundary-begin/Pending, `]` for Boundary-end, `·` for Landmark —
ASCII/Latin-1 single-width (no ambiguous-width risk), styled with the same faces as their
cell forms. The cell is appended AFTER the glyph runs in `row_spans_placed` (before
`paint_rows`' fold-marker suffix), mirroring the established painted-span idiom.

Call-site plumbing: `row_spans_placed` gains a `last_row: bool` parameter, supplied by
`paint_rows` as `row_index + 1 == visual_rows.len()` (both values already in scope
there); on the last row it calls `ctx.block_paint.trailing_marker(editor, l)` (which
computes `end_line`/`cend` internally — `editor` carries `theme`/`depth`/the view).
`row_spans_segs` is untouched — a row with any landmark takes the placed path by
`wants_placed`.

Known limitation (accepted, probed at execute — §10.1): on a visual row exactly
`text_width` columns wide the appended cell may be clipped by the row `Rect`. The mark
remains discoverable via the status segments and jumps; the probe documents actual
behavior, including against B17's phantom flush row (where a trailing-space wrap already
produces an extra flush row — if the anchor's `last_row` is that flush row, the marker
rides it and clipping does not arise).

### 3.4 Status segments (`render_status.rs::status_left_text`)

Extending the existing BLK match (§2.3), with two new appends after it:

- **Direction (4B):** `Some(b)` non-hidden → `" · BLK"` + `block_paint::blk_direction`,
  which returns `"↑"` when `b.end <= lo`, `"↓"` when `b.start >= hi`, else `""`.
  `lo` = `derive::line_start(buf, view.scroll)`; `hi` = the end of the last laid-out
  line's coverage, mirroring the search-window upper bound in `gather_row_ctx`
  (`line_layouts` keys → max line → `ventilate::resolve(...).last_line` →
  `derive::line_start(buf, last_line + 1)`). Line-granular by construction (the partially
  scrolled first line counts as visible) — a deliberate approximation for a hint; if
  `line_layouts` is empty the direction is `""`. `BLK·hidden` keeps its exact current
  form (no arrow — a hidden block paints nothing, the arrow would promise a landmark the
  canvas doesn't show).
- **Pending (2C):** `pending_block_begin.is_some()` → append `" · BLK…"`. Independent of
  the block segment (both can show: re-marking while a block exists is legal —
  `block_begin` sets the anchor without touching `marked_block`).
- **Mark identity (sub-fork A):** `block_paint::marks_on_caret_line` — caret line =
  `buffer.byte_to_line(nav::head(editor))`; collect `(name, pos)` from `marks` where
  `byte_to_line(pos)` equals it; BTreeMap iteration gives deterministic name order;
  format `"MK 3,a"` → appended as `" · MK 3,a"`. `None` (no marks on the line) appends
  nothing. O(#marks) per frame.

`…`, `·`, `↑`, `↓` are typographic single-width characters, consistent with the existing
status vocabulary (`· BLK·hidden`, menu `…` labels); the no-emoji house rule concerns
emoji, not these.

### 3.5 C2 — `clear_mark` / `clear_marks`

- `editor.rs`: `pub enum MarkPending { Set, Jump, Clear }` (+`Clear`). The only
  variant-exhaustive consumer is `marks::resolve_pending` (verified by grep;
  `marks::intercept` matches on the message, not the variant).
- `marks.rs`:
  - `pub fn clear_mark(editor: &mut Editor)` — `pending_mark = Some(MarkPending::Clear)`,
    status `"clear mark:"` (mirrors `set_mark`/`jump_to_mark`).
  - `resolve_pending` gains the `Clear` arm: `marks.remove(&ch)` → status
    `"mark {ch} cleared"` / `"no mark {ch}"`.
  - `pub fn clear_marks(editor: &mut Editor)` — `let n = marks.len()`; clear; status
    `"{n} marks cleared"` (or `"no marks"` when `n == 0`).
- `registry.rs`, beside the existing `set_mark`/`jump_to_mark` rows:

```rust
r.register("clear_mark",  "Clear Mark\u{2026}", None, |c| { crate::marks::clear_mark(c.editor);  CommandResult::Handled });
r.register("clear_marks", "Clear All Marks",    None, |c| { crate::marks::clear_marks(c.editor); CommandResult::Handled });
```

  `register` (not `register_mut`): marks are buffer metadata, not document content —
  identical to `set_mark`/`set_bookmark_N`, which are usable on read-only buffers today.
  `MenuCategory::None` — every existing mark command is palette-only; ④ follows the
  established curation (§5). No default keybinding (the ^K chord space is allocated;
  the palette is the keyboard path, and a user config patch binding gets hints for free).

### 3.6 Theme changes (`wordcartel-core/src/theme.rs`)

New variants (doc-commented, placed after `MarkedBlock`):

```rust
/// The begin/end boundary cells of a marked block (and the pending ^KB anchor cell).
MarkedBlockBoundary,
/// A char-mark / numbered-bookmark presence cell (landmark visibility, Effort ④).
LandmarkGlyph,
```

Mechanical, compiler-forced updates: `Faces` fields `marked_block_boundary` /
`landmark_glyph`; `face()` / `face_mut()` arms; `element_from_key` keys
`"marked_block_boundary"` / `"landmark_glyph"` (user theme overrides work immediately);
`ALL_ELEMENTS` 35 → 37 (+ its count comment); `face_requirement`:
`MarkedBlockBoundary | LandmarkGlyph => Modifier`. `MarkedBlock` STAYS class `Highlight`
(every RGB interior keeps its bg; `no_color` is non-Rgb and Part-B-exempt).

**The per-theme face table** (the seven constructor sites; the two new faces are
UNIFORM modifier-only literals everywhere, so they survive every depth including cue
mode; the interior edit is "keep bg, drop the modifier stack"):

| Constructor (themes) | `marked_block` interior — NEW (was: same bg + reverse+bold+underline) | `marked_block_boundary` | `landmark_glyph` |
|---|---|---|---|
| `default()` (default, terminal-plain) | `bg: DarkGray` | reverse+bold+underline | reverse+italic+underline |
| `tokyo_night()` | `bg: DARK3` | reverse+bold+underline | reverse+italic+underline |
| `terminal_ansi()` | `bg: DarkGray` | reverse+bold+underline | reverse+italic+underline |
| `blue_jeans(name, r)` (dark/dusk/paper) | `bg: r.mark_bg` | reverse+bold+underline | reverse+italic+underline |
| `from_base16(name, p)` (10 themes) | `bg: b[0x3]` | reverse+bold+underline | reverse+italic+underline |
| `no_color()` | `reverse+dim` (mono — no bg) | reverse+bold+underline | reverse+italic+underline |
| `phosphor(name, hue)` (5 themes) | `bg: shade(hue, 2)` | reverse+bold+underline | reverse+italic+underline |

Mono cue-space audit (the reason for exclusive classification, decision 8) — the
`no_color()` combos after ④, all pairwise distinct among co-occurring elements:
interior `r+d` (unique; comment is `i+d`, search_match is `r`), boundary `r+b+u`
(inherits the retired interior combo), landmark `r+i+u` (selection `r+u`, front_matter
`r+i`, prose_lens `b+i+u`, diag_grammar `i+u` — all differ by exactly the distinguishing
modifier). Because classification is exclusive, no landmark face ever accumulates onto
another (in particular `DIM|BOLD` — notoriously terminal-inconsistent — cannot arise
from ④'s own faces).

**Everything is additive** — new faces only SET fg/bg/modifiers; nothing needs to clear
an inherited modifier. `face_to_ratatui`'s add-only shape is sufficient; **H25 stays
out of ④.**

Contract-test updates:
- `marked_block_mono_modifier_is_distinct` is REWRITTEN to pin the new allocation:
  interior `(reverse, dim) == (Some(true), Some(true))` with bold/underline unset;
  boundary `(reverse, bold, underline)` all set; landmark `(reverse, italic, underline)`
  all set; pairwise `assert_ne!` across
  {interior, boundary, landmark, Selection, SearchMatch, SearchCurrent, DiagSpelling,
  DiagGrammar, ProseLensMatch, FrontMatter, Comment}.
- `every_rgb_builtin_satisfies_the_completeness_contract` and
  `face_is_total_and_heading_clamps` extend automatically via `ALL_ELEMENTS` /
  `face_requirement`.
- `prose_lens_bg_distinct_from_marked_block_in_color_themes` stays green unmodified
  (every interior bg is unchanged).

Depth note (Selection precedent): at `Depth::None` a bg-only RGB interior face renders
nothing — exactly like tokyo's bg-only `selection` face today. Cue-mode users are served
by the mono themes, whose interior carries `reverse+dim`; the boundary and landmark cells
(modifier-only) remain visible at every depth in every theme.

### 3.7 render.rs integration (complete list of hub edits)

1. `gather_row_ctx`: block snapshot lines → `crate::block_paint::gather(editor)`;
   `use_placed` term `has_block` → `block_paint.wants_placed()`; `RowCtx.marked_block`
   field → `RowCtx.block_paint`.
2. `row_spans_placed`: the 9-line block arm (§2.3) → one
   `style = ctx.block_paint.patch_glyph(style, g_from, g_to, &editor.theme, editor.depth);`
   line; after the run-flush, ~three lines:
   `if last_row { if let Some(sp) = ctx.block_paint.trailing_marker(editor, l) {
   spans.push(sp); } }`; signature gains `last_row: bool`.
3. `paint_rows`: passes `row_index + 1 == visual_rows.len()` at the single
   `row_spans_placed` call site.
4. `overlaps` stays where it is; `block_paint` calls it as
   `crate::render::overlaps` (it is `pub(crate)`).

No other render.rs edits. Net production-line delta is negative (§6).

---

## 4. Visible behavior changes (deliberate, user-facing)

### 4.1 The block looks different (approved)

Today: a completed block is a full-span reversed+bold+underlined highlight (reads as a
shouting selection). After ④: a quiet tinted interior with two strong boundary cells —
a landmark, not a selection. `hidden` semantics unchanged (suppresses ALL block paint;
status still `· BLK·hidden`). The `select_marked_block` bridge, ops, and jumps are
untouched.

### 4.2 Newly-visible pre-existing behaviors (documented, not changed)

- **Undo drift** (locked decision 9): after undo, a painted mark sits at its last-mapped
  offset against the reverted text until the next jump/edit re-clamps/remaps it. This was
  always true; ④ makes it visible. Not a defect; recorded here as the authoritative
  statement of intent.
- **Concealed/folded marker bytes** paint nothing until revealed (§3.2/§3.3).
- A restored session's block returns visible (`hidden` is not persisted) — unchanged,
  but now visibly so.

---

## 5. Command-surface contract — conformance (ENGAGES)

Per `docs/design/command-surface-contract.md`:

- **Law 1 (registry = single source of truth):** `clear_mark`/`clear_marks` are
  registered `registry.rs` builtins dispatching to `marks.rs` bodies; no side-channel
  dispatch. The interactive slot capture reuses the EXISTING `pending_mark` intercept —
  the same mechanism `set_mark`/`jump_to_mark` already route through.
- **Law 2 (every option is a command):** ④ adds NO user-settable option (visibility is
  always-on; no `SettingsSnapshot`/config key) — no obligation arises.
- **Law 3 (palette exhaustive):** both commands are non-hidden registry entries → they
  appear in the palette by construction (the palette enumerates the registry); the
  existing palette-completeness invariant covers them with zero new test code.
- **Law 4 (menu ⊆ palette):** `MenuCategory::None` — the menu is unchanged. Judgment
  call recorded: ALL 24 existing mark commands are palette-only; the marks' home is the
  palette, and ④ follows it rather than inventing a menu section for two commands.
  (`MenuCategory::Block` was considered and rejected: these act on marks, not the block.)
- **Law 5 (keyboard path):** the palette is the keyboard path; no default chord is bound
  (the ^K space is fully allocated — §2.2's keymap grounding).
- **Laws 6/8/9 (setters/multi-state):** N/A — no option, no state cycle.
- **Law 7 (hints track the active keymap):** generic — with no default binding the
  palette shows no chord; a user patch binding (`Keymap::bind_user`-backed config patch)
  surfaces automatically in hints, as for any command.
- **Law 10 (plugin spine):** both commands are nullary and registry-dispatched — plugin-
  callable like every other command.

---

## 6. Anti-regrowth: GATE conformance accounting

- **`module_budgets` — render.rs 899/900 at `6067ad5`.** ④'s hub edits (§3.7): gather
  collapse (−3), block-arm replacement (−8, +1), trailing-marker call (+3), `last_row`
  plumbing (±1). **Net ≈ −6 → ~893/900.** The spec REQUIRES the executed diff to leave
  render.rs production strictly BELOW 899 (assert by running the budget test; no budget
  bump is permitted for ④).
- **app.rs 1000-cap:** untouched (zero ④ edits in app.rs; C2 lands in registry.rs +
  marks.rs, neither budgeted).
- **`clippy::too_many_lines` (100):** every new `block_paint.rs` function is small
  (gather ~10, classify ~20, patch_glyph ~12, trailing_marker ~25, status helpers ~15
  each); no `#[allow]` anticipated.
- **Registration seam doctrine:** new behavior enters via a feature module invoked by
  thin arms + two registry rows — the exact pattern the module-structure rule mandates.
- Standard GATEs: `cargo test` all suites, `cargo build`/`test --no-run` warning-free,
  workspace clippy clean. PTY smoke suite run + verbatim summary at pre-merge
  (advisory). `cargo fmt` never run (house style).

---

## 7. Test plan (contracts; TDD detail belongs to the plan)

**New: `block_paint.rs` unit tests**
- classification priority at shared cells: mark at `b.start` → Boundary wins; pending at
  a mark position → Pending wins; one-glyph block → Boundary (no interior).
- exclusivity: interior cells get ONLY the interior face (no boundary modifiers).
- `wants_placed`: false when empty; true for each of block / pending / marks alone;
  hidden block alone → false.
- gather: sorted+deduped landmarks; hidden filter.
- trailing rule (the §3.3 vectors, verbatim): `"abc\n"` mark at 3 fires; `"**a**\n"`
  mark at 3 (concealed) does NOT fire AND paints no cell anywhere; `"**a**\n"` mark at 5
  fires after the visible `a`; empty line fires; final-line-no-newline fires at `len`;
  `"é"` mark at EOF (byte 2) fires WITHOUT panicking (the multibyte-EOF round-2 vector);
  block `[0,4)` over `"abc\n"` yields the trailing `]`; soft-wrap boundary byte does NOT
  fire on the non-final row; priority yields one cell.

**wordcartel-core `buffer.rs`**
- `TextBuffer::byte`: doc-comment `# Examples` block (house rule for public items) plus
  a unit assert on a multibyte buffer (`"é"`: `byte(0) == 0xC3`, `byte(1) == 0xA9`) —
  the mid-codepoint read that motivates the accessor.
- `blk_direction`: above / below / partially visible / empty-layout cases.
- `marks_on_caret_line`: multi-mark ordering `3,a`; empty → None.

**render.rs / TestBackend tests (the "render the screen" discipline)**
- pending ^KB paints a boundary-faced cell at the anchor (the B12 pin — RED first
  against today's code); EOL anchor paints the trailing `[`.
- completed block: boundary cells carry REVERSED+BOLD+UNDERLINED; interior cells carry
  the tint bg WITHOUT those modifiers (asserts the §4.1 look change; the existing
  `marked_block_paints_and_status_shows_blk` keeps passing — its `row_has_highlight`
  finds the boundary modifiers — and is complemented, not replaced).
- `hidden_block_status_reads_blk_hidden_and_not_painted` stays green (gather filter).
- a mark paints a LandmarkGlyph-faced cell; a mark on a fold-hidden line paints nothing.
- status: `BLK…` while pending; `BLK↑`/`BLK↓` with the block scrolled off each way;
  `MK 3,a` on the caret line; all absent otherwise.

**theme.rs**
- rewritten `marked_block_mono_modifier_is_distinct` (§3.6); totality/completeness
  auto-extended; `prose_lens_bg_distinct…` untouched-green.

**marks.rs / registry**
- `clear_mark` interactive round-trip (Set 'a' → Clear 'a' → jump fails with
  "no mark a"); `clear_marks` count status; `resolve_pending` Clear arm on an absent
  name; both commands present in the palette (covered by the palette-completeness
  invariant — add no bespoke test).

**e2e (`wordcartel/src/e2e.rs`)** — one journey: ^KB → see the pending cell + `BLK…` →
^KK → boundaries + tint + `BLK` → scroll away → `BLK↓`/`BLK↑` → `^Q b` back.

---

## 8. Out of scope (filed / follow-on)

- **Multi-BLOCK model** (Vec/named/ring) — rejected at fork time; positions cover the
  multi-landmark intent.
- **Landmark gutter** (per-line tick/digit column) + identity-at-a-glance: a LAYOUT
  feature (narrows `text_width` → global reflow + mouse/cursor remap) — the priced
  follow-on, judged after living with inline cells.
- **Mark-management overlay** (list + delete) — C3, follow-on.
- **Visibility toggle option** — only if speckle proves real in live use.
- **H25** (compose modifier subtraction) — untouched; ④ is fully additive.
- **B-full injected marker cells** — per the settled cursor-review §3.2 decision.

## 9. Backlog bookkeeping (at merge)

`backlog.toml`: B12 + B13 → shipped (④'s merge); prose sections move to
`docs/backlog-archive.md` with `doc =` repointed; `scripts/backlog bless`. H25 remains
open (its prose already stands alone). A new triage item may be filed for the landmark
gutter follow-on via `scripts/backlog add` if the human wants it tracked.

## 10. Residual claims — verify at execute (cannot be proven by reading)

1. **Trailing-marker clipping on exact-width rows.** A `text_width`-wide visual row plus
   an appended marker cell may be clipped by the row `Rect` in `paint_rows`
   (`Paragraph` truncation). Needs a TestBackend probe at exact-width geometry,
   including interaction with B17's phantom flush row (trailing-space-at-margin). The
   spec ACCEPTS clipping if confirmed (status + jumps keep the mark discoverable) — the
   probe pins whichever behavior is real. (The concealed-markup soundness axis of this
   flag was RESOLVED at the Codex round-1 fold — the corrected §3.3 predicate tests the
   logical content end, not the visible `src_span` end, with the `**a**` vector pinned
   in unit tests; only the clipping/B17 axis remains for the execute-time probe.)
   **OUTCOME (T7 probe, `render.rs::trailing_marker_exact_width_and_flush_row_probe`):**
   confirmed clipping on the exact-width row (a 10-col row of 10 visible chars leaves the
   appended marker span one cell past `row_area`'s right edge; ratatui's `Paragraph`
   truncates at the buffer boundary — no corruption, no panic, no stray styling on the
   row, the marker simply doesn't render). ACCEPTED per this section — status (`MK …`)
   and jump navigation remain the mark's discovery path on that row. B17 interaction is
   the opposite of a hazard: a hung trailing space wraps its line to a flush continuation
   row (B17), and that flush row — otherwise empty — becomes the trailing marker's
   `last_row`; the glyph paints cleanly at its column 0 with the full `LandmarkGlyph`
   face, no clipping. Both branches are now locked test pins, not open questions.
2. **Live-terminal legibility of the chosen faces** (A21 lesson: synthetic buffers prove
   composition, not readability). A tui-interact eyeball pass during execute on
   tokyo-night, one phosphor, terminal-plain, and no-color: interior tint vs canvas,
   boundary reverse+bold+underline, landmark italic rendering (some terminals fake or
   drop italic), `reverse+dim` interaction in mono mode.
3. **`byte_to_line` cost on the status path** is assumed O(log n) (rope line index) —
   consistent with its ubiquitous per-frame use elsewhere (`fold`, `nav`); flagged only
   for honesty, no probe needed.

## History

- 2026-07-24 — drafted for the Codex spec gate (Fable warm thread, effort ④).
- 2026-07-24 — Codex spec-gate round 1 fold: corrected the trailing edge-marker
  predicate (Important). The draft's `m == row_hi` test read `VisualRow::src_span`,
  which spans VISIBLE content only, so a mark on a concealed trailing markup byte
  (`**a**` byte 3) would have painted a misleading EOL cell. Now tests
  `m == logical_content_end(buf, end_line)` (buffer-derived line boundary via the
  clamping `derive::line_start`), with the `**a**` / `abc\n` vectors pinned (§3.3).
  All other gate findings: GO.
- 2026-07-24 — Codex spec-gate round 2 fold: UTF-8 panic hazard in
  `logical_content_end` (Important). The round-1 helper's `slice(next - 1..next)`
  trips `TextBuffer::slice`'s char-boundary assert when the final line ends in a
  multibyte char (`"é"`, `next == len == 2`). The newline test is now a byte-level
  read via a new one-line core accessor `TextBuffer::byte` (delegates to
  `ropey::Rope::byte`, ropey 1.6.1); `0x0A` can never be a UTF-8 continuation byte, so
  the test is unambiguous. Multibyte-EOF vector added (§3.3/§7); audited: no other
  spot in ④ slices at a possibly-non-boundary offset. Trailing-marker correctness
  itself: confirmed resolved at round 2.
