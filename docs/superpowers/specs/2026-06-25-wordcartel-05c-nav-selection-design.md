# Effort 5c — Keyboard Navigation & Selection (design)

**Status:** design / pre-plan
**Date:** 2026-06-25
**Depends on:** 5a (keymap trie + presets + session/marks store), 5b (registry metadata + palette/menu), 4r (Buffer extraction), core block_tree + selection + layout.

## 1. Goal & scope

Give the editor *prose-grade* navigation and selection beyond single-grapheme arrow motion: word/paragraph/page/document movement, expand-by-scope selection (word→sentence→paragraph→document), named marks, and an automatic position-history ("take me back") ring.

**In scope (5c):**
- Word / paragraph / page / document keyboard navigation (+ word-wise delete).
- Text objects exposed as **expand/shrink selection** commands (no modal grammar).
- **Named letter marks** (`m{a-z}` set / jump), persisted via the 5a store.
- **Jump-back ring** (`Alt+Left`/`Alt+Right` through prior positions).
- The `cell → byte-offset` reverse map (inverse of `nav::screen_pos`).

**Out of scope (deferred):**
- **Mouse** (click / drag-select / wheel / double-click-word / triple-click-paragraph) and a draggable scrollbar → **Effort 5c-m**, which reuses this effort's segmentation substrate *and* the cell→offset reverse map landed here.
- Vim operator-pending text-object grammar (`d i p` etc.) — not pursued; expand/shrink covers the prose need.
- Search / go-to-line (5e), outline jumps (5g).

## 2. Crate & widget posture

No new dependencies. Everything 5c needs is already present:
- **`unicode-segmentation = "1"`** (already in `wordcartel-core`) — UAX-#29 word *and* sentence boundaries for word nav, word-delete, and the word/sentence text objects.
- **Paragraph boundaries** come from the existing **`wordcartel-core::block_tree`** (a paragraph *is* a block), so navigation and text objects agree with the document model rather than re-deriving blank-line scans.
- `crossterm` / `ratatui` — unchanged; no mouse capture is enabled in 5c (that is 5c-m). The optional `ratatui::Scrollbar` widget is a 5c-m concern.

## 3. Architecture & modules

| Unit | Responsibility | Depends on |
|------|----------------|-----------|
| **`wordcartel-core/src/textobj.rs`** (new, pure) | Word/sentence boundary queries over a `&str` window; the prose substrate. `#![forbid(unsafe_code)]`, deterministic, oracle-tested. | `unicode-segmentation` |
| `wordcartel-core/src/block_tree.rs` (existing) | Source for paragraph spans — `nav::paragraph_range_at` recurses to the nearest *leaf* block (§4), with a blank-line gap fallback. | — |
| **`wordcartel/src/nav.rs`** (extend) | Word/paragraph/page/document caret motions (consume `textobj` + block tree + layout). `cell → offset` reverse map. | core textobj, layout, block_tree |
| **`wordcartel/src/marks.rs`** (new) | Per-buffer marks map + jump ring; set/jump/back/forward command bodies; edit-mapping; persistence sync to `state.rs`. | core selection map, state.rs |
| `wordcartel/src/editor.rs` (extend) | `Buffer` gains `marks`, `jump_ring`, `ring_cursor`, `sel_history`; `Editor` gains `pending_mark`. Map marks/ring in `Buffer::apply`. | — |
| `wordcartel/src/registry.rs` / `keymap.rs` (extend) | Register the new commands; add CUA + WordStar bindings. | 5a/5b |
| `wordcartel/src/app.rs` (extend) | `pending_mark` interception block; reduce wiring; `ensure_visible` after every motion. | — |

**Why this split:** the pure boundary logic is the part most worth isolating and exhaustively testing (oracle tests against hand-verified strings); the shell stitches those bounds across block boundaries and into commands.

## 4. Core `textobj.rs` — boundary substrate (pure)

Operates on a **`text: &str` window with `pos` relative to that window** — the shell passes the *containing block's slice* (cheap: one block span) so work is bounded by a paragraph, never the whole document (responsiveness: word motion must not materialize an N-MB string). The shell translates returned offsets by the block start, and stitches across block boundaries itself (mirroring how `move_right` stitches across logical lines today).

```rust
//! Pure word/sentence boundary queries (UAX-#29). Offsets are byte indices
//! into `text`; `pos` is clamped into `0..=text.len()`.

/// (from, to) byte range of the word at `pos`. If `pos` is in inter-word
/// whitespace, returns the zero-width point (pos, pos) — the caller decides
/// whether to extend (text objects extend; motion skips).
pub fn word_bounds(text: &str, pos: usize) -> (usize, usize);

/// Start of the next word strictly after `pos`, or `None` if none remain in
/// this window (caller then advances to the next block's first word).
pub fn next_word_start(text: &str, pos: usize) -> Option<usize>;

/// Start of the word before `pos`, or `None` if at/the first word of the
/// window (caller then crosses to the previous block's last word).
pub fn prev_word_start(text: &str, pos: usize) -> Option<usize>;

/// (from, to) of the sentence containing `pos`, scoped to this window
/// (the shell passes the paragraph block as the window).
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize);
```

`word_bounds`/`next`/`prev` use `split_word_bound_indices`; a "word" is any segment whose first char is alphanumeric (UAX-#29 keeps apostrophe contractions like `don't` as one segment and splits hyphenates like `well-known` — accepted prose behavior, pinned by tests). Punctuation/whitespace runs are non-words: **motion skips them**; **text objects** that start in whitespace select the *nearest* word — next word within the block, else the previous (the tie rule, tested explicitly). `sentence_bounds` uses `split_sentence_bound_indices` over the window.

**Paragraph bounds (`nav::paragraph_range_at(blocks, buf, pos) -> (from, to)`)** are computed in `nav.rs`, **not** `textobj`, and must be *total* over the document (block spans are sparse and nested — Codex spec-review):
- **Recursive descent to the nearest *leaf* block:** walk `blocks.top_level()` and recurse into children, returning the span of the *deepest* block whose `span` contains `pos` (so a list *item* / blockquote *paragraph* is selected, not the whole `List`/`BlockQuote` container).
- **Gap fallback:** if `pos` falls in an inter-block gap (blank lines, ref-def gaps, past-EOF) where no block span contains it, return the **maximal run of non-blank logical lines** around `pos` (blank-line-delimited; empty range if `pos` is on a blank line). This makes the function defined everywhere.

**The window passed to `textobj` for word/sentence queries is this same leaf-block (or gap) span**, so a sentence never spans across list items or block boundaries.

## 5. Keyboard navigation

Each is a discrete registered command (palette/menu-visible via 5b; category **View** or none). All call `nav::ensure_visible` + clear/preserve `desired_col` per the existing motion contract. Selection variants set `head` while keeping `anchor` (extend); plain variants collapse to a point.

| Command | Recommended chord (free; plan verifies) | Behavior |
|---|---|---|
| `move_word_left` / `_right` | `ctrl-left` / `ctrl-right` | Prev/next word start; cross block boundary at window edge. |
| `select_word_left` / `_right` | `ctrl-shift-left` / `ctrl-shift-right` | Same, extending the selection. |
| `move_paragraph_up` / `_down` | `ctrl-up` / `ctrl-down` | Prev/next paragraph (block) start. |
| `move_page_up` / `_down` | `pageup` / `pagedown` | By editing-area height − 1 visual row (overlap), wrap-aware via layout. |
| `move_doc_start` / `_end` | `ctrl-home` / `ctrl-end` | Buffer start / end. |
| `delete_word_back` / `_forward` | `ctrl-backspace` / `ctrl-del` | Delete from caret to prev/next word boundary (one undo step). |

Cross-block stitching: when `next_word_start` returns `None` at the block's end, the motion lands on the first word of the next block (or doc end); symmetric for `prev`. `move_paragraph_*` jump between block spans. `delete_word_*` reuse the same boundary to compute the deletion range, then go through the normal `Buffer::apply` edit channel (so undo/redo, mark-mapping, and recovery all work).

## 6. Text objects — expand / shrink selection

Five commands, all built on §4/§5 bounds:

| Command | Recommended chord | Behavior |
|---|---|---|
| `select_word` | (palette) | Select the word at the caret. |
| `select_sentence` | (palette) | Select the sentence at the caret. |
| `select_paragraph` | (palette) | Select the paragraph (block) at the caret. |
| `expand_selection` | `ctrl-w` | Grow the selection to the next larger scope: **word → sentence → paragraph → document**. |
| `shrink_selection` | `ctrl-shift-w` | Reverse the last expand. |

**Mechanism — selection-history stack (per `Buffer`):** `expand_selection` pushes the current `Selection` onto `sel_history` and replaces it with the smallest scope strictly larger than the current selection (computed from the caret's word/sentence/paragraph/document bounds; if the current selection already equals one scope, jump to the next). `shrink_selection` pops `sel_history` back. Empty start: `expand_selection` from a point begins at `word`.

**Reset rule (Codex spec-review — must cover non-command mutation paths):** the ladder is invalidated by *any* change that isn't an expand/shrink, and several mutation paths bypass the command layer (paste at `app.rs`, filter/transform results, direct `Buffer::apply` callers). So `sel_history` is cleared in **two places**: (1) inside **`Buffer::apply`** (covers every edit — typing, paste, filter, transform, delete-word — in one spot) and (2) at the top of every **caret-motion command except `expand_selection`/`shrink_selection`** (arrows, word/para/page/doc, mark-jump, ring). Only the two ladder commands preserve it. `undo`/`redo` go through neither, so they also clear it explicitly.

## 7. Marks & jump-back ring

### 7.1 Named marks
- **State:** `Buffer.marks: BTreeMap<char, usize>` (in-session; mirrors the persisted 5a `BTreeMap<String, usize>`).
- **Capture model:** `set_mark` and `jump_to_mark` do **not** hard-bind 52 trie leaves. Each sets `Editor.pending_mark = Some(MarkPending::{Set|Jump})`; the **next key press** is consumed as the mark name (`a`–`z`, `0`–`9`). This reuses the editor's existing modal-interception precedent (minibuffer / overlays). Esc cancels; any prompt/overlay open clears it (§9).
  - `set_mark` + char → `marks.insert(char, caret_offset)`; status `"mark {c} set"`.
  - `jump_to_mark` + char → if present, **push the current caret to the ring** (§7.2), set selection to the stored offset (clamped to a valid boundary), `ensure_visible`; else status `"no mark {c}"`.
- **Recommended triggers (plan finalizes):** `set_mark` and `jump_to_mark` are palette-accessible regardless of binding. Correction (Codex spec-review): **`ctrl-k` is a *WordStar* prefix, not CUA** — in CUA `ctrl-k m`/`ctrl-k j` are free, but in the **WordStar** preset any `ctrl-k …` mark trigger must not shadow the existing `ctrl-k ctrl-s/q/c/v` family (use a distinct trailing key). Suggested: CUA `ctrl-k m` (set) / `ctrl-k j` (jump); WordStar maps to its block-mark family. A multi-key trigger composes with capture: the trie resolves `ctrl-k m` → dispatches `set_mark` (clearing `pending_keys`) → the interception block captures the *next* raw key as the name.

### 7.2 Jump-back ring
**State (per `Buffer`):** `jump_ring: Vec<usize>` (bounded `CAP = 64`) + `ring_cursor: usize` (index of the "current" position within the ring; `== jump_ring.len()` means "at the live caret, past the newest entry"). Precise invariants (Codex spec-review — pin the state machine so implementations don't diverge):

- **What pushes** (`record_jump(pre_jump_offset)`): only **deliberate jumps** — `jump_to_mark`, `move_doc_start`/`_end`. Continuous motion (arrows, word, paragraph, page) does **not** push.
- **Push algorithm:** (1) if `ring_cursor < len` (we had stepped back), **truncate** `jump_ring[ring_cursor..]` (drop the stale forward tail); (2) **duplicate-suppress** — if the last entry equals `pre_jump_offset`, don't push; (3) push `pre_jump_offset`; (4) if `len > CAP`, drop the front (oldest); (5) set `ring_cursor = len` (parked at the live caret).
- **`jump_back` (`alt-left`):** if `ring_cursor == len`, first **append the current caret** as the forward anchor (so forward can return to "now") and treat cursor as `len` over the appended list; then if `ring_cursor > 0`, `ring_cursor -= 1` and move the caret to `jump_ring[ring_cursor]` (clamped+grapheme-snapped). No-op at `ring_cursor == 0`.
- **`jump_forward` (`alt-right`):** if `ring_cursor + 1 < len`, `ring_cursor += 1` and move to `jump_ring[ring_cursor]`. No-op at the newest entry.
- Both `ensure_visible`; both are non-expand motions so they clear `sel_history` (§6). At ring ends → no-op + status hint. The ring is **per-buffer, session-only** (Effort 6 may later globalize it across buffers).

### 7.3 Persistence
The `StateEntry.marks` *shape* exists from 5a, but the load/save paths currently **discard** marks (Codex spec-review: `app.rs apply_resume` returns only cursor/scroll; the persist path writes `marks: BTreeMap::new()`). **5c must wire both ends:**
- **Save:** on the existing `saved_version`-watch persist, write `Buffer.marks` (char→single-char `String`) into `StateEntry.marks` instead of an empty map.
- **Load:** extend `apply_resume` to populate `Buffer.marks` from the loaded entry, **only when 5a's mtime+size staleness guard accepts the entry** (same gate as cursor/scroll); each offset **clamped to `0..=len` and grapheme-snapped** before it becomes usable.
- The **ring is session-only** (not persisted).

## 8. Edit-tracking contract (decision: live mapping)

Marks and ring entries are **byte offsets that must follow the text**, exactly as `Selection` already does:

- **Extract the shared mapper first (task-ordering dependency):** `map_pos` is currently *private* in `wordcartel-core::selection` (used by `Selection::map`). Promote it to a public `wordcartel-core::change::map_pos(pos: usize, cs: &ChangeSet) -> usize` and have `Selection::map` call it. This refactor MUST land **before** the marks-mapping task or marks will duplicate the logic.
- **Forward edits — capture the `ChangeSet` *before* it is consumed (Codex Critical):** `Buffer::apply` (editor.rs:77) moves `txn` into `history.commit_coalescing(txn, …)` (which takes the `Transaction` by value and moves `txn.changes` into history), so the ChangeSet is gone after the commit. Fix: **clone the ChangeSet before the commit** — `let cs = txn.changes.clone();` (the public `Transaction.changes` field; `ChangeSet` derives `Clone`) — then after `commit_coalescing` returns the mapped selection, map every `marks` value and every `jump_ring` entry through `&cs` with `change::map_pos`. The clone is ≤ a few ops and cheap. Also clear `sel_history` here (§6). Cost is trivial (≤ ~90 offsets per keystroke; far below the responsiveness budget).
- **Undo / redo:** marks/ring are **not** re-mapped through inverse changesets (history restores only the selection — editor.rs:86-97). Instead they are **clamped on use**. Deliberate, documented approximation — precise mark tracking across undo is low value, high complexity.
- **Clamp-on-use / on-load means clamp *and grapheme-snap*** (Codex Minor): `min(offset, len)` alone can leave an offset mid-UTF-8 and panic later in a slice path. Every consumption of a mark/ring offset (jump, back/forward, load) clamps to `0..=len` **then** snaps to a grapheme boundary (via the layout `snap_to_stop` already used by nav).

## 9. Interception / XOR discipline

`pending_mark` is a one-keystroke capture state. It sits **above** the overlay blocks in `reduce` (it resolves on the very next key): when `Some`, a printable key press is consumed as the mark name and `pending_mark` is cleared; `Esc` cancels; any **non-key message falls through** (so async results still apply while capture is pending). For invariant cleanliness it joins the single-active-modal discipline from 5b: the helpers `open_prompt`/`open_minibuffer`/`open_palette` clear `pending_mark`, **and — since the menu is opened by a *manual toggle* in `registry.rs` with no `open_menu` helper (Codex spec-review) — the `menu` command body must clear `pending_mark` itself**, alongside the other modal fields it already clears. `pending_mark` capture clears nothing else (it is transient) and does not persist across an overlay open. It is distinct from the 5a keymap-trie `pending_keys` buffer: the trie resolves a full sequence and clears `pending_keys` *before* dispatch, so a multi-key trigger (`ctrl-k m`) and the subsequent raw-char capture never contend.

## 10. The `cell → offset` reverse map (lands here for 5c-m)

Add `nav::offset_at_cell(editor, col: u16, row: u16) -> Option<usize>` — the inverse of `screen_pos`: walk visible logical lines from `(scroll, scroll_row)` accumulating visual rows to find the line+visual-row under `row`, then `ColMap::visual_to_source(vrow, col)` (+ the existing `snap_to_stop`) → global offset, clamped to line end. Built and unit-tested in 5c (it pairs naturally with `screen_pos`); **no caller wires it to mouse events until 5c-m**, so it carries a scoped `#[allow(dead_code)] // wired in 5c-m`.

## 11. Error handling & edge cases

- Empty buffer / single empty line: all motions clamp to offset 0; text objects select the empty range; no panic.
- Caret in inter-word whitespace: `word_bounds` returns a point; `select_word` then selects the *nearest* word (next within block, else prev).
- Multi-byte / grapheme: all returned offsets snap to grapheme boundaries via the layout `snap_to_stop` before becoming a caret (mirrors existing nav).
- Block boundaries: word/paragraph motion at the last block lands at doc end and is a no-op past it; `delete_word_forward` at doc end is a no-op.
- Mark name not `a-z`/`0-9` (e.g. a function key while `pending_mark`): cancel capture, no mark set, status hint.
- `expand_selection` at document scope: no-op (already maximal); `shrink_selection` with empty history: no-op.
- Ring at either end: no-op with a brief status hint.

## 12. Testing strategy

- **Core (`textobj.rs`) — oracle/unit:** hand-verified `(text, pos) → bounds`/`next`/`prev` over ASCII, multi-byte (`é`, CJK), punctuation, em-dashes, and sentence cases (`"Dr. Smith went. Home."`), including empty/edge windows. These are pure and deterministic — the highest-value tests.
- **Shell `nav.rs`:** word motion crossing block boundaries; paragraph motion across headings/lists/blank gaps; page motion on wrapped lines (reuse the existing wrapped-line fixtures); doc start/end; `delete_word_*` producing one undo step; `offset_at_cell` round-trips with `screen_pos`.
- **Shell `marks.rs` / app:** set→edit-above→jump lands on the *mapped* offset (proves §8 live tracking); jump pushes the ring and `jump_back` returns; capture state consumes exactly one key and Esc cancels; persistence round-trips marks through `state.rs` with the staleness guard.
- **Selection ladder:** expand word→sentence→paragraph→document then shrink back; a non-expand command resets the ladder.
- No pre-existing test weakened; full workspace stays green, zero warnings.

## 13. Module & command summary

- **New files:** `wordcartel-core/src/textobj.rs`, `wordcartel/src/marks.rs`.
- **New commands:** `move_word_left/right`, `select_word_left/right`, `move_paragraph_up/down`, `move_page_up/down`, `move_doc_start/end`, `delete_word_back/forward`, `select_word/sentence/paragraph`, `expand_selection`, `shrink_selection`, `set_mark`, `jump_to_mark`, `jump_back`, `jump_forward`.
- **Editor state added:** `Buffer.marks`, `Buffer.jump_ring`, `Buffer.ring_cursor`, `Buffer.sel_history`; `Editor.pending_mark`.
- **Shared refactor:** extract `change::map_pos` so `Selection` and marks share one offset-mapping implementation (DRY, §8).

**Task-ordering prerequisites (Codex spec-review):**
1. Extract `change::map_pos` (and repoint `Selection::map`) **before** any marks-mapping task.
2. Add the per-`Buffer` mark/ring/sel_history fields **and** the `Buffer::apply` change — clone `txn.changes` before `commit_coalescing`, map marks/ring, clear `sel_history` — as the foundation the mark/ring/expand commands build on.
3. `nav::paragraph_range_at` (leaf-block recursion + gap fallback) is a dependency of both `move_paragraph_*` and `select_paragraph`/`expand_selection`.
4. Wiring marks load/save into `state.rs`/`apply_resume` is its own task gated by the staleness guard.

## 14. Deliberate decisions (for review)

1. **Live edit-tracking** of marks/ring (forward), **clamp-on-use** across undo/redo — §8.
2. **Paragraph = block-tree block**, not a blank-line re-scan — §2/§4.
3. **Expand/shrink** model for text objects, **no vim operator grammar** — §6.
4. **Mouse split to 5c-m**, but the `cell→offset` reverse map lands in 5c — §1/§10.
5. **Pending-char capture** for mark naming instead of 52 trie leaves — §7.1.
6. Word motion/text objects work on the **containing block window**, not the whole document, for responsiveness — §4.
