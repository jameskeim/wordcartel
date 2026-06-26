# Wordcartel Effort 5g — Outline & Folding — Design

**Date:** 2026-06-26
**Status:** Design (pre-plan)
**Effort:** 5g (after 5f Harper diagnostics) — the last Effort-5 sub-effort

## 1. Summary

Structure navigation and section folding driven by the document's **heading
hierarchy** (already computed by the block-tree). Three user-facing pieces:

- **Outline picker (A):** a fuzzy, transient overlay listing every heading
  (indented by level); Enter jumps to it and pushes the jump-ring. Reuses the
  palette / `nucleo-matcher` / jump-ring stack.
- **Heading motions (B):** standalone `heading_next` / `heading_prev` /
  `heading_parent` commands over the heading list — normal keymap dispatch, **no
  overlay**, independent of A.
- **Section folding:** collapse a heading's body (down to the next same-or-higher
  heading) to navigate document shape. Folds **survive edits** (anchors remapped
  through `ChangeSet` like marks), the **cursor can't land inside a fold**, folds
  **persist across sessions** (re-anchored on reopen), and jumping into a folded
  section **auto-unfolds** the ancestor chain.

The heading extraction is a pure `wordcartel-core::outline` module. Folding's
keystone is a **single fold-aware visible-line iterator** (`fold::visible_lines`)
that every line-space consumer routes through: `derive::rebuild` omits folded
body lines from the layout cache, AND the nav on-demand layout paths, mouse
hit-testing, scrollbar, typewriter, page motion, and doc-end motion are all
re-expressed over visible lines. Hidden lines do **not** "fall out for free" —
the Codex spec review found several consumers (`nav::get_or_layout` /
`layout_line_active` / `layout_line_on_demand`, `mouse::offset_at_cell`,
the scrollbar ratio, the typewriter solver) that compute line positions
*independently* of the cache and would re-materialize or mis-index hidden lines.
The deliberate, tested surface is therefore **every consumer that walks logical
lines**, all funneled through one visible-line API (§4).

## 2. Goals / Non-Goals

### Goals
- `outline::headings` + `outline::section_range` — pure, block-tree-derived.
- Outline overlay (A): fuzzy-filter, level-indented, Enter-jumps, jump-ring, auto-unfold-to-target.
- Heading motions (B): next / prev / parent, standalone, jump-ring, fold-aware.
- Section-by-heading folding: `fold_toggle` / `fold_all` / `unfold_all`.
- Folds survive edits (anchor remap via `map_pos`, like marks).
- Cursor never inside a fold; folding the caret's section moves the caret to the heading.
- Fold display: `▸` marker + dim `… N lines` hint on the folded heading line; body suppressed.
- Cross-session persistence via `StateEntry.folds`, re-anchored on reopen.
- Composes with 5d (measure/typewriter/focus) and 5f (diagnostics): hidden rows
  are absent from paint, and the fold-affected consumers (typewriter centering,
  focus region, diagnostic/search caret jumps) are explicitly fold-aware (§4.2, §5.3).

### Non-Goals (v1)
- **Block folding** (code blocks / lists / blockquotes) — section-by-heading only.
- **Fold-to-level-N** (fold all `##`, etc.) — only toggle / all / none.
- A persistent **fold-gutter column** — inline marker only (no width cost).
- A persistent **outline side panel** — overlay picker only (panel is Effort-6 multi-pane territory).
- Editing *inside* a fold (auto-expand-on-type) — you unfold to edit; the cursor can't enter a fold.
- Folding by non-heading structure (lists/blockquotes).

## 3. Architecture

Functional-core / imperative-shell.

```
wordcartel-core (IO/thread-free, #![forbid(unsafe_code)])
  outline.rs  NEW — Heading{level,byte,text}; headings(&BlockTree,&rope)->Vec<Heading>;
                    section_range(&BlockTree, heading_byte)->Range<usize>;
                    heading_starts(&BlockTree,&rope)->BTreeSet<usize>; recursive
                    pre-order traversal (into containers) + ATX/setext title strip. Pure.

wordcartel (shell)
  fold.rs        NEW — FoldState{folded: BTreeSet<usize>}; toggle/all/none;
                       hidden_byte_ranges + reconcile; AND the §4.0 visible-line
                       API (hidden_line_ranges, next/prev_visible_line,
                       visible_line_count, visible-ordinal mapping, normalize
                       caret/scroll) that every consumer routes through.
  outline_overlay.rs NEW — OutlineOverlay{buffer_id,..} (fuzzy heading picker) state.
  editor.rs      + per-Buffer `folds: FoldState` (on Buffer, beside marks/jump_ring);
                       `outline: Option<OutlineOverlay>` + open_outline() XOR clear;
                       Buffer::apply remaps folds (Before-biased); undo/redo reconcile.
  derive.rs      + the fold-skip in rebuild's walk + normalize scroll (KEYSTONE §4.1).
  nav.rs         + fold-aware: on-demand layout refuses hidden lines; vertical/
                       horizontal motion over visible lines; ensure_visible/scroll
                       snapping; typewriter row accounting; page step; doc-end
                       normalize; offset_at_cell over visible lines (§4.2).
  mouse.rs       + scrollbar drag + hit mapping over visible-line count (§4.2.6).
  render.rs      + fold marker (▸ / … N lines) on folded heading rows; scrollbar
                       ratio over visible-line count; overlay paint.
  registry.rs    + commands: outline (open A), heading_next/prev/parent (B),
                       fold_toggle/fold_all/unfold_all; diag-nav auto-unfold (§5.3).
  app.rs         + outline overlay interception + non-key fallthrough; auto-unfold
                       on jump AND on search/replace hit (§5.3); persist/resume folds.
  input.rs/keymap.rs + key binds (resolved in the plan against the current keymap).
  state.rs       + StateEntry.folds: Vec<usize> (#[serde(default)]); save/restore + reconcile.
  save.rs        + reconcile folds + normalize caret on reload/recovery (§7).
```

### 3.1 Core: `wordcartel-core::outline`

```rust
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heading {
    pub level: u8,       // 1..=6 (from BlockKind::Heading(level))
    pub byte: usize,     // byte offset of the heading's start (block.span.start)
    pub text: String,    // the heading's display text (title only, markers stripped)
}

/// All headings in document order (pre-order over the block tree).
pub fn headings(blocks: &BlockTree, rope: &Rope) -> Vec<Heading>;

/// The fold span of the heading whose start is `heading_byte`: from that heading
/// to the start of the next heading with level <= this heading's level, or end
/// of document. Returns the FULL section range (heading line + body); callers
/// hide `heading_line_end..range.end` (the body), keeping the heading visible.
pub fn section_range(blocks: &BlockTree, heading_byte: usize) -> std::ops::Range<usize>;

/// The canonical set of heading-start byte offsets. `FoldState::reconcile`
/// validates anchors against THIS set, not `block_tree::role_at` (which only
/// classifies a byte's role and cannot prove a byte is a heading *start*).
pub fn heading_starts(blocks: &BlockTree, rope: &Rope) -> std::collections::BTreeSet<usize>;
```

**Codex-review corrections (verified against the real `block_tree.rs`):**
- `block_tree` exposes only `top_level()` and `role_at()` — there is **no
  pre-order heading-traversal API** today. `outline.rs` owns a recursive
  pre-order walk over `root.children` that descends into containers
  (blockquote/list), so headings nested inside containers are found.
- `block_tree` does **not** provide heading-title extraction; block spans come
  straight from pulldown offsets. `outline.rs` owns title stripping for **both
  ATX (`## Title`) and setext (`Title\n===`)** headings, returning the title
  text only.
- `reconcile` uses `heading_starts()` (canonical start offsets), never
  `role_at()`, to decide whether an anchor still lands on a heading start.
- Byte offsets come **only** from rope/block APIs (never synthesized);
  `TextBuffer::slice` asserts char boundaries, so titles are sliced on real
  boundaries. Multibyte titles/offsets are explicitly tested.

Pure: derived from the existing `block_tree` (`BlockKind::Heading(u8)`,
`Block{span,children}`). Oracle/unit-tested on fixed documents (nesting,
same-level siblings, deepest level, no-headings, heading at EOF, setext
headings, headings inside a blockquote/list, multibyte titles).

### 3.2 Shell: `FoldState` + the visible-line model

```rust
// per-Buffer (editor.rs)
pub struct FoldState {
    pub folded: std::collections::BTreeSet<usize>, // byte offsets of folded headings
}
impl FoldState {
    /// Hidden body ranges in BYTES given the current block tree. For each folded
    /// heading still present, hide its body (heading_line_end .. section_range.end).
    /// Folds whose offset is no longer a heading are ignored here (and pruned by
    /// reconcile()).
    pub fn hidden_byte_ranges(&self, blocks: &BlockTree, rope: &Rope) -> Vec<Range<usize>>;
    pub fn toggle(&mut self, heading_byte: usize);
    pub fn fold_all(&mut self, blocks: &BlockTree, rope: &Rope);   // fold every heading
    pub fn unfold_all(&mut self);                                  // clear
    /// Drop anchors that no longer start a heading (after edits / reopen),
    /// validated against `outline::heading_starts`.
    pub fn reconcile(&mut self, blocks: &BlockTree, rope: &Rope);
}
```

The **layout-facing** surface (the line-space iterator the keystone routes every
consumer through — `hidden_line_ranges`, `next/prev_visible_line`,
`visible_line_count`, `normalize_caret/scroll`, ordinal mapping) lives in
`fold.rs` and is derived from `hidden_byte_ranges` + the rope's byte→line map.
See §4.0.

- **Anchor remap (survive edits):** `FoldState` lives on `Buffer` (alongside
  `marks`/`jump_ring`) so `Buffer::apply` can remap it. Remap each `folded`
  offset through the committed `ChangeSet` — but **not** with the default
  `change::map_pos`, which has insertion bias **After** (verified at
  `change.rs:123-141`). A heading-start anchor must stay *before* text inserted
  at its own offset, or a paste exactly at the heading start would push the
  anchor into the body and `reconcile` would drop the fold. So fold anchors use a
  **Before-biased** remap (either a `map_pos_before` variant or `map_pos` with an
  explicit bias arg — chosen in the plan). Tested with insertion at heading byte
  0 and at a mid-document heading start.
- **Undo/redo (Codex-review gap):** `Buffer::undo`/`redo` today replace the
  document content but do **not** remap `marks`/`jump_ring` (verified at
  `editor.rs:102-112`). Folds must not silently break across undo/redo: treat
  undo/redo as a **full reconcile** of `FoldState` against the restored
  block-tree (drop anchors that no longer land on a heading start; surviving
  anchors are re-validated by offset). This is specified as new work, not assumed
  to exist.
- After any remap or reconcile, `reconcile()` prunes any anchor that no longer
  lands on a heading start (via `outline::heading_starts`).
- Fold spans are **recomputed each frame** from the live block-tree (not stored),
  so editing above a fold (which shifts the heading) Just Works once the anchor
  is remapped.

## 4. The keystone — fold-aware layout (§ the high-bug-surface part)

Today everything is in **logical-line space**: `view.scroll` is a dense
logical-line index, `derive::rebuild` walks logical lines into a
`BTreeMap<line,(rows,map)>`, and render / nav / mouse read that cache — **but the
Codex spec review proved they do NOT read it exclusively.** Several consumers
compute line positions independently and would re-materialize or mis-index hidden
lines. So folding's keystone is **not** "omit lines from the cache and the rest
falls out." It is a **single fold-aware visible-line API** that every line-space
consumer is rerouted through.

### 4.0 The shared visible-line API (`fold.rs`)
One source of truth, derived each frame from `FoldState` + the block-tree:
- `hidden_line_ranges(blocks, rope) -> Vec<Range<usize>>` — hidden *logical-line*
  ranges (each folded heading's body: `heading_line_end .. section_range.end`).
- `is_line_hidden(line) -> bool`.
- `next_visible_line(line) -> Option<usize>` / `prev_visible_line(line)` — the
  fold-skipping successor/predecessor used by **all** vertical movement.
- `visible_line_count(total) -> usize` — total minus hidden (scrollbar + paging).
- `visible_ordinal_of(line)` / `line_at_visible_ordinal(n)` — map between a dense
  "nth visible line" and its logical-line index (scrollbar drag, page step).
- `normalize_caret(byte) -> byte` / `normalize_scroll(line) -> line` — snap a
  byte/line that fell inside a hidden range to the owning folded **heading**.

### 4.1 `derive::rebuild` skips folded bodies (the producer)
In the `while l < total_lines` walk (derive.rs:115-143): compute the hidden
line-ranges once. The walk starts at `view.scroll`, so **first normalize
`view.scroll` through `normalize_scroll`** (a fold can swallow the previous top).
For each `l`:
- if `l` is a **folded heading line** → lay it out (render adds the `▸`/`… N`
  marker), then **advance `l` to `next_visible_line` past the fold body**;
- else lay out normally.

The body lines are **absent from the cache**, which fixes *paint*. The rest of
this section fixes every consumer that does **not** rely solely on the cache.

### 4.2 The deliberate consumers (each rerouted through §4.0 + tested)

Codex enumerated these against the real code; all are in scope:

1. **nav on-demand layout (Critical, `nav.rs:55-69,142-149`):**
   `get_or_layout` / `layout_line_active` / `layout_line_on_demand` lay out a
   missing line *on demand* — they would happily re-materialize a folded body
   line that `rebuild` skipped. Each must **refuse hidden lines** (return None /
   the heading's layout) so absent ≠ recomputed.
2. **Vertical / horizontal motion (Critical, `nav.rs:284-365`):** movement walks
   dense `l+1` / `l-1`. Reroute through `next_visible_line` / `prev_visible_line`
   so a folded heading is a single stop and the caret can't enter the body (same
   contract as concealed markers, §16 master design).
3. **`view.scroll` model (Critical, `editor.rs:48-56`, `nav.rs:404-540`):**
   `scroll` is a dense logical index in `ensure_visible` (clamps to total),
   row accounting (`(scroll+1)..caret_line`), and one-row scroll. **Decision
   (committed, not optional): `view.scroll` remains a *logical-line* index but is
   invariably a *visible* line** — every write to it passes through
   `normalize_scroll`, and one-row scroll steps via `next/prev_visible_line`.
   (Rejected the "visible ordinal" representation: it would touch every existing
   scroll read; the normalize-on-write rule is the smaller, safer change.)
4. **`ensure_visible` (Critical):** never pin caret or scroll to a hidden line;
   clamp against `visible_line_count`, step via visible-line helpers.
5. **Mouse hit-testing (Critical, `nav.rs:749-776`):** `offset_at_cell` iterates
   `line += 1` over `total_logical_lines` and lays out on demand → clicks could
   target a folded body. Reroute over **visible lines only**, same iterator.
6. **Scrollbar + drag (Important, `render.rs:382-386`, `mouse.rs:139-150,
   203-216`):** ratio and drag use total logical lines. Replace with
   `visible_line_count` + `line_at_visible_ordinal`.
7. **Typewriter solver (Important, `nav.rs:379-402`):** centering walks `0..l`
   and `0..total` logical lines — folded body rows still affect centering. Add
   fold-aware row accounting (visible lines only).
8. **Page up/down (Important, `nav.rs:665-687`):** call `move_up`/`move_down`
   repeatedly → inherit dense landing. Fixed transitively once (2) skips hidden
   ranges; page step uses `visible_line_count`.
9. **Doc-end / Home-End (Important, `nav.rs:650-655`):** `move_to_doc_end`
   returns raw `buf.len()`, which can be inside a folded final body. Apply
   `normalize_caret` after the motion.
10. **Caret-normalize-after-motion (invariant):** every motion/jump ends with
    `normalize_caret` (snap into-fold landings to the owning heading), giving a
    single enforced "caret always visible" invariant rather than per-path fixes.

> This is still the 5d "one source, many consumers" keystone — but the review
> showed the consumer list is **10, not 3**, and the source is an explicit
> `fold.rs` API, not the cache's absence. Each consumer gets a test.

## 5. Outline picker (A) & motions (B)

### 5.1 Picker (A)
- Open command `outline`; a transient overlay (palette rectangle helper) lists
  `headings()` indented by `level`, fuzzy-filtered with `nucleo-matcher`
  (the existing palette stack). ↑/↓ select, Enter jump, Esc cancel.
- **Jump = auto-unfold ancestors + move caret + push jump-ring + ensure_visible.**
  If the target heading is inside a folded ancestor, unfold the ancestor chain so
  the target line is visible before moving the caret. (`Ctrl+O` returns via the
  jump-ring, 5c.)
- **Overlay model (Codex-review correction):** there is **no single XOR overlay
  slot** — `Editor` holds independent `Option` fields (`prompt`, `minibuffer`,
  `palette`, `menu`, `search`, `diag`; verified `editor.rs:146-185`) with XOR
  enforced manually in each opener (`editor.rs:251-323`). Add
  `outline: Option<OutlineOverlay>` plus an `open_outline()` method that clears
  **every** other overlay (both directions, matching the existing openers).
  `OutlineOverlay` carries an explicit `buffer_id` (mirroring
  `SearchState::buffer_id` / `DiagOverlay::buffer_id`) and no-ops / closes if the
  active buffer changes.
- Insert outline interception **beside** the existing palette/search/diag blocks
  in `reduce` (`app.rs:735-818,900-980`), which already have key-only handling +
  **non-key fallthrough** (the 5e/5f starvation lesson); the plan adds the outline
  branch with explicit non-key-fallthrough tests so it can't starve background
  messages or steal priority.

### 5.2 Motions (B) — standalone
- `heading_next`: nearest heading with `byte > caret`; `heading_prev`: nearest
  with `byte < caret`; `heading_parent`: nearest preceding heading with
  `level < current section's level`. Each sets the caret to the heading byte,
  pushes the jump-ring, `ensure_visible`. **No overlay; works with A never
  opened.** Headings stay visible even when folded, so motions land fine; a
  motion to a heading hidden inside a folded ANCESTOR auto-unfolds that ancestor.

### 5.3 Fold composition with existing nav features (Codex-review gaps)

Folding adds hidden ranges that several *existing* commands can jump a caret
into. Each gets an explicit, tested policy:

- **Search / replace-stepping (Critical, `app.rs:464-480,577-581`):** a search
  hit can land inside a folded body and today relies only on `ensure_visible`,
  which would place the caret in a hidden line. Policy: **auto-unfold the
  ancestor chain of a search/replace match** before moving the caret (consistent
  with the picker/motion auto-unfold), then `ensure_visible`. Tested for both
  incremental search and query-replace stepping.
- **Diagnostics next/prev + quick-fix (Important, `registry.rs:221-245`):**
  `F8`/`Shift+F8`/quick-fix set the caret by byte and can target a hidden
  diagnostic. Policy: **auto-unfold to the diagnostic** before moving the caret
  (same as search). Hidden diagnostics simply aren't painted (their rows are
  absent), but navigating to one reveals it.
- **Focus dim (5d) (Important, `render.rs:191-210,270-277`):** the active
  paragraph/sentence region is computed from the caret's byte span and painted
  over visible rows. Because the caret is normalized out of folds (§4.2.10) it is
  always on a visible heading/line, so the active region is always paintable; the
  folded heading row uses the normal active/dim rule for the heading line itself
  (its hidden body contributes no rows).

## 6. UI / keys / rendering

### 6.1 Fold display
- A folded heading row renders its heading text plus a **leading `▸`** and a dim
  trailing **`… N lines`** (N = hidden logical-line count). Expanded headings get
  an optional subtle `▾` or nothing (default: nothing, to stay clean).
- No fold-gutter column (width-free; text-first).
- The marker composes with the existing concealed-markdown / focus-dim / search /
  diagnostic styling at the cache layer.

### 6.2 Keys (resolved in the plan against the current keymap)
| Action | Intent (exact bind chosen in the plan against the fuller keymap) |
|--------|----------|
| open outline picker | e.g. `Ctrl+J` or `Alt+O` (`Ctrl+G`=word-count is taken) |
| next / prev heading | e.g. `Alt+↓` / `Alt+↑` |
| parent heading | e.g. `Alt+←` |
| fold toggle (at cursor) | e.g. `Alt+Z` or `Ctrl+,` |
| fold all / unfold all | e.g. `Alt+Shift+Z` / `Alt+Shift+X` |

> Key selection is a **plan task** — the keymap gained `Ctrl+F/R`, `F3`,
> `Ctrl+.`, `F8` in 5e/5f. The plan picks free binds in the CUA preset (+ the
> `input.rs` test mirror) and re-confirms no collision; provide palette-reachable
> fallbacks for any terminal-fragile chord.

## 7. Persistence & reconciliation

- `StateEntry` (state.rs) gains `folds: Vec<usize>` (folded heading byte-offsets),
  saved alongside the existing `cursor` / `scroll` / `marks` / `mtime` / `size` /
  `seq` fields, and wired through `persist_session` (`app.rs:1480-1489`) and the
  resume path (`app.rs:1319-1327`).
- **Serde migration (Codex-review, Important):** `folds` MUST be
  `#[serde(default)]` — `load_in` silently returns an empty session on any TOML
  parse failure (`state.rs:79-83`), so a non-default new field would wipe every
  user's saved session the first time they upgrade. A round-trip test loads an
  old `session.toml` (no `folds` key) and asserts it deserializes with
  `folds == []`.
- **On resume:** the existing **file-identity check (mtime + size)**
  (`app.rs:246-257`, `state.rs:95-104`) already discards a `StateEntry` when the
  file changed on disk — so a changed file resets folds cleanly (no stale-fold
  surprise). When identity matches, restore `folds` into `FoldState`, then
  **`reconcile()`** against the freshly parsed block-tree (drop any offset that
  isn't a heading start — defensive even on a match), **before** the first
  `ensure_visible` / `rebuild`.
- **On reload/recovery** (save.rs, the buffer-replacement sites
  `save.rs:122-155,160-183`, like 5f's DiagStore reset): **reconcile** the
  existing `FoldState` against the new buffer (anchors that still land on a
  heading start survive; others drop). Then `normalize_caret` keeps the cursor
  visible. (Decision: reconcile, not clear — a reload of the same file on disk
  should preserve the user's folds where the headings still exist.)

## 8. Error handling / edge cases

| Situation | Behavior |
|-----------|----------|
| No headings in the doc | outline picker shows "no headings"; motions/fold are no-ops |
| Fold a heading with empty body (heading immediately followed by another) | nothing to hide → toggle is a no-op (or folds 0 lines, marker still shows) |
| Caret inside a section being folded | caret moves to the heading line |
| Edit deletes a folded heading | its anchor maps to the deletion point; `reconcile` drops the fold |
| `scroll` lands inside a new fold | snapped to the fold heading (ensure_visible) |
| Jump (picker/motion) into a folded ancestor | ancestor chain auto-unfolds before the caret moves |
| Heading at EOF | `section_range` ends at doc end; folds the trailing body (possibly empty) |
| Restored fold offset (reopen) no longer a heading | dropped by `reconcile` |
| Folded ranges + typewriter/focus/diagnostics | compose at the cache layer (hidden lines simply absent) |

## 9. Performance / responsiveness (the #1 priority)

- `headings()` / `section_range()` are O(blocks) over the already-built block-tree
  — cheap; computed on demand (picker open, motion, fold toggle), not per
  keystroke.
- `FoldState::hidden_byte_ranges` is O(folds) and computed once per
  `derive::rebuild` (which is already the per-frame layout pass) — the fold-skip
  makes rebuild lay out *fewer* lines, never more.
- Anchor remap is O(folds) per edit (like marks) — negligible.
- No new thread, channel, or async; no change to the input-loop latency model.

## 10. Testing

### 10.1 Core (`outline`)
- `headings`: nesting / sibling levels / deepest level / no-headings / EOF heading;
  **setext headings**; **headings nested in a blockquote/list** (container
  descent); text extraction strips ATX/setext markers; byte offsets correct
  (multibyte titles).
- `section_range`: `##` stops at next `##` or `#`; `###` stops at next `###`/`##`/`#`;
  last heading → doc end; nested children included.
- `heading_starts`: exactly the heading-start offsets (the set `reconcile` uses).

### 10.2 Shell
- **FoldState:** toggle/all/none; `hidden_byte_ranges` correct vs the block-tree;
  anchor remap through an edit above a fold keeps the fold on the right heading;
  **Before-biased remap** keeps the anchor on the heading when text is inserted
  *exactly at* the heading start (byte 0 and mid-doc); `reconcile` drops a
  deleted-heading anchor; **undo/redo reconcile** keeps/drops folds correctly.
- **Visible-line API (§4.0):** `hidden_line_ranges`, `next/prev_visible_line`,
  `visible_line_count`, ordinal↔line mapping, `normalize_caret/scroll` — unit
  tested directly.
- **Keystone consumers (§4.2), each tested:** `derive::rebuild` omits folded body
  lines (cache has the heading, not the body) and normalizes a swallowed scroll;
  nav on-demand layout **refuses** a hidden line (absent ≠ recomputed); vertical/
  horizontal motion treats a fold as one stop (lands heading, then next visible);
  `ensure_visible`/scroll never pin to a hidden line; **mouse** `offset_at_cell`
  click into a folded region resolves to the heading, not the body; scrollbar
  ratio + drag use visible-line count; typewriter centering ignores hidden rows;
  page up/down + doc-end land on visible lines.
- **Fold composition (§5.3):** search/replace hit inside a fold auto-unfolds
  ancestors before the caret moves (incremental + query-replace); diag-next /
  quick-fix into a hidden diagnostic auto-unfolds; focus-dim active region paints
  with the caret normalized out of folds.
- **Outline A:** picker lists headings level-indented; fuzzy filter; Enter jumps +
  auto-unfolds ancestors + pushes jump-ring; `open_outline` XOR-clears every other
  overlay; outline reduce branch lets non-key messages fall through; stale
  `buffer_id` no-ops.
- **Motions B:** next/prev/parent correct + standalone (A never opened); jump-ring
  pushed; motion into a folded ancestor auto-unfolds.
- **Render:** folded heading shows `▸` + `… N lines`; expanded shows none;
  no-fold/no-overlay = true no-op (existing render tests unchanged).
- **Persistence:** `StateEntry.folds` round-trips; **old `session.toml` with no
  `folds` key deserializes to `folds == []`** (serde-default migration); reopen
  with matching identity restores + reconciles; changed-file identity mismatch
  resets folds; reload/recovery reconciles + normalizes caret.

## 11. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| The logical-line↔visible-line mapping is pervasive — Codex found **10** independent consumers (nav on-demand layout, vertical/horizontal motion, scroll, ensure_visible, mouse hit-test, scrollbar+drag, typewriter, page, doc-end), not 3 | **Reframed keystone:** one `fold.rs` visible-line API (§4.0); every consumer routes through it; each gets a test. The cache's absence fixes only *paint* — the API fixes the rest |
| Caret/scroll landing inside a hidden range | single invariant "caret + scroll always visible" enforced by `normalize_caret` after every motion/jump and `normalize_scroll` on every scroll write; tested |
| Fold anchors going stale on edit | **Before-biased** remap (not default `map_pos`, which is After-biased) + `reconcile` against `heading_starts`; undo/redo reconcile |
| Reopen restoring stale folds for a changed file | the existing mtime+size identity check discards the entry; reconcile on match; `#[serde(default)]` folds so old sessions don't fail to load |
| Search/diag/quick-fix jumping the caret into a fold | auto-unfold ancestors before the caret moves (§5.3); tested for search, query-replace, diag-nav |
| Scrollbar/PgUp-Dn proportion wrong under folds | use `visible_line_count` + visible-ordinal mapping (committed; no optional fallback) |
| Key collisions (keymap is fuller after 5e/5f) | a plan task picks free CUA binds + test mirror + palette fallbacks |

## 12. Out of scope → future
- Block folding (code/list/blockquote); fold-to-level-N; persistent outline panel
  or fold gutter — noted as likely follow-ups once the section-fold machinery is
  proven. (Search/diagnostic *navigation* into folds **is** in v1 scope, §5.3 —
  a match jump auto-unfolds; only a persistent fold gutter/panel is deferred.)
