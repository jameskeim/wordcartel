# Wordcartel Effort 9A — Persistent Marked Blocks — Design

**Status:** design (brainstormed 2026-06-28)
**Roadmap:** Effort 9A, exec #4, pre-1.0 (`docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`).
**Goal:** add the WordStar "mark now, act later" primitive — a **persistent, range-valued,
edit-tracking** marked block, **separate** from the live CUA selection, that survives cursor
movement, edits, and (Effort 9A decision) save/close/reopen, and is acted on later from
anywhere: copy/move/delete/write-to-file/jump/hide. The signature differentiator.

---

## 1. Scope & philosophy

- A **single** persistent marked block per buffer (WordStar-faithful — "mark *a* block").
  Multiple blocks are out of scope (clean later extension).
- **Distinct from the live selection.** WordStar had no live selection — the marked block
  *was* its selection. wordcartel keeps its modern CUA `document.selection` (shift-select,
  type-to-replace, fluid extend) **and** layers the persistent block alongside it. They are
  two coexisting primitives with **distinct visual cues** (§7).
- All commands register through the §10.4 name-keyed registry → **palette-reachable in every
  preset**. WordStar-preset gets the `^K`/`^Q` bindings; CUA gets only the **promote** bridge
  bound (§8). All text mutations route through the single `editor.apply` edit channel (undo +
  edit-tracking + plugin substrate).
- **No `wordcartel-core` change beyond a new `SemanticElement::MarkedBlock`** (theme) — the
  block state and ops live in the shell.

---

## 2. Data model & edit-tracking

On `Buffer` (editor.rs, beside `marks`/`jump_ring`):
```rust
pub struct MarkedBlock { pub start: usize, pub end: usize, pub hidden: bool } // start <= end
// pub marked_block: Option<MarkedBlock>,          // a COMPLETE block
// pub pending_block_begin: Option<usize>,         // half-set ^KB-without-^KK state
```
- **Edit-tracking is free:** add `marked_block.{start,end}` and `pending_block_begin` to the
  existing `Buffer::apply` loop that maps `marks`/`jump_ring` through `change::map_pos`
  (editor.rs:162). Both endpoints map via `map_pos` (same as `Selection::anchor`/`head`), so
  the block: **moves** on an external insert/delete, **grows** on an insert inside it,
  **shrinks** on a delete inside it.
- **Collapse → clear:** after mapping, if `start == end` (the region was fully deleted) →
  `marked_block = None`. A half-set `pending_block_begin` that maps onto a deletion just
  tracks; it is dropped on completion/replacement.

---

## 3. Creation

- **`^KB`** (`block_begin`): `pending_block_begin = Some(caret)`. (Does not clear an existing
  `marked_block` until `^KK` completes a new one — matches WordStar re-marking.)
- **`^KK`** (`block_end`): if `pending_block_begin` is set → `marked_block =
  Some(normalize(begin, caret))` (start = min, end = max), clear pending. If pending is `None`
  → status "set block begin first" (no-op).
- **Promote** (`mark_block_from_selection`): if the live selection is non-empty →
  `marked_block = Some({sel.from(), sel.to(), hidden:false})`, clear `pending_block_begin`,
  and **clear the live selection** (`Selection::single(caret)`) — the selection has been
  *converted* into the persistent block. Empty selection → status "no selection to mark".
- **Reject an empty block.** If a creation path would produce `start == end` (`^KK` at the
  begin position, promote of a zero-width selection), do **not** set a block → status
  "empty block" (no momentary empty block; complements the §2 collapse→clear rule).
- A new `^KB`+`^KK` or a promote **replaces** any existing block.

---

## 4. Operations (block = SOURCE, caret = TARGET; own channel, not the `^C`/`^V` register)

Each requires a complete `marked_block` (else status "no marked block"). Text mutations go
through `editor.apply` (one undo step each):
- **`^KC` copy** (`block_copy`): insert `buffer.slice(start..end)` at the caret; the caret
  lands at the **end of the inserted text**. **Block stays** (its endpoints map through the
  insertion via `apply`). Repeatable.
- **`^KV` move** (`block_move`): build a **single `ChangeSet`** that both deletes the original
  `start..end` **and** inserts the block text at the caret — one undoable edit; the caret
  lands at the **end of the moved text**. **Block cleared** after. **Guard:** if the caret is
  **inside** `[start, end)` → no-op + status "can't move a block into itself".
- **`^KY` delete** (`block_delete`): delete `start..end`; **block cleared**.
- **`^QB` / `^QK` jump** (`block_jump_begin`/`block_jump_end`): move the caret to `start`/`end`,
  **fold-aware** (`place_caret_visible(UnfoldTo)`) and **records a jump-back**
  (`marks::record_jump`), exactly like the goto/mark jumps.
- **`^KH` hide/show** (`block_toggle_hidden`): toggle `marked_block.hidden`. The block still
  acts; only its highlight is suppressed (§7).
- **clear-block** (`block_clear`): `marked_block = None`, `pending_block_begin = None`.

**Move ordering note:** `block_move` must construct the ChangeSet so the insert and delete
offsets are mutually consistent (the classic "delete original then insert at adjusted target",
or one composed ChangeSet over the original document). The plan specifies the exact
construction; the contract is: the block text ends up at the caret, the original is gone, and
it is **one** undo step.

**Undo interaction (consistency, not a bug):** the `marked_block` is *editor state*, not part
of the undo transaction — exactly like `marks` and the live `Selection`. `apply` (including an
undo, which is itself an `apply` of the inverse changeset) **maps** the block endpoints via
`map_pos` but does **not** restore a block that was *cleared*. So `^KY` (or an edit that
collapses the block) followed by **undo** brings the text back but **not** the block. This
matches the existing marks/selection behavior; it is documented, not special-cased.

**No system-clipboard sync (deliberate):** `^KC`/`^KV` are the block's **own** channel — they
insert directly at the caret and do **not** populate the OS clipboard / OSC-52 register. A
user who wants the system clipboard uses the live selection + `copy`. (Intentional scope
choice, stated so it is not read as an omission.)

---

## 5. `^KW` write block → file

`block_write` reuses Effort 7's Save-As minibuffer infrastructure:
- A **`MinibufferKind::WriteBlock`** prompt ("Write block to: ", pre-filled with the doc's
  dir / cwd) → resolve path (`~`/relative expansion, as Save-As) → if the target exists, an
  **overwrite confirm** (a new `PromptAction::OverwriteWriteBlock` + `pending_write_block:
  Option<PathBuf>`, mirroring `OverwriteSaveAs`/`pending_save_overwrite`) → `file::save_atomic`
  of the **block text** (`slice(start..end)`).
- **No** document `path`/`stored_fp`/`saved_version` change and **no swap re-key** — this
  writes a *separate* file; the current document is untouched. The block **stays**.
- Status "wrote block to {path}" / on error the `save_atomic` error string.

---

## 6. Persistence across sessions (decision: persist)

The block joins the per-file session state (`state.rs`), restored on reopen by Effort 7's
`restore_resume`, under the **same mtime+size staleness guard** that protects the resumed
cursor/marks/folds:
- `StateEntry` gains `#[serde(default)] block: Option<(usize, usize)>` (serde-defaulted so
  pre-9A `session.toml` loads). Only a **complete** `marked_block` is persisted (start, end);
  the half-set `pending_block_begin` is **not** persisted.
- **Persist:** wherever the session entry is built on save/exit (alongside cursor/marks/folds),
  record `block = marked_block.map(|b| (b.start, b.end))`.
- **Restore** (in `restore_resume`): if the staleness guard passes and `entry.block` is set →
  `marked_block = Some({start, end, hidden:false})` (**`hidden` resets to visible on reload**).
  If the file changed on disk (guard fails) → the whole entry (incl. block) is discarded — we
  never act on stale byte offsets.
- **Cadence:** the block persists on the **same save/exit path** as cursor/marks/folds — it is
  **not** flushed on an in-app buffer-switch (Effort 7 open-replace). So a block that was
  marked but never saved is lost if you open another file in-app. Acceptable for single-buffer
  9A; revisit when Effort 6 makes per-buffer state switch-driven.

---

## 7. Visual cue (§13.2-safe)

- New **`SemanticElement::MarkedBlock`** + a `Face` field on `ThemeFaces`. Every theme
  constructor that builds faces (the 13 builtins, `from_base16`, `face`/`face_mut`,
  `element_from_key`, the a11y/coverage test lists) gains a `MarkedBlock` default — **distinct
  from `Selection`**.
- **Monochrome-safe (§13.2):** `Selection` is **reverse**; `MarkedBlock` is **reverse +
  italic**. This must be **pairwise-distinct** from every other cued element under the
  theming a11y proof — notably `Selection` (reverse alone), the search faces, and the
  diagnostics (which already use **underline**-based modifiers: spelling = bold+underline,
  grammar = italic+underline). `reverse + italic` is distinct from all of these. In color:
  a tinted background **plus** the `reverse + italic` modifiers (so the cue survives at
  `Depth::None`). `MarkedBlock` MUST be added to the a11y pairwise-distinct test set.
- **Render layering:** the block paints as a **backdrop below** Selection/Search/Diag (active
  things win where they overlap), via the same placed-path compose layering the theming work
  established (base → MarkedBlock → Selection → Search → Diag). `hidden == true` → not painted.
  Overlap is normally absent (the block is elsewhere from the caret/selection; promote clears
  the selection).
- **Status-bar presence indicator.** A marked block can be entirely **off-screen**, with no
  on-screen signal that one is armed. So when `marked_block` is set, the status line shows a
  small **`BLK`** segment (themed via `SemanticElement::Chrome`, like the other status
  segments), riding the existing status-line assembly (`render.rs`, alongside `Ln, Col` /
  word-count). When the block is **hidden** (`^KH`), the indicator reads **`BLK·hidden`** (the
  block still exists and acts — the indicator is how you know). No block → no segment.

---

## 8. Keybindings

- **WordStar preset** (`keymap.rs` `WORDSTAR`): `^KB`→`block_begin`, `^KK`→`block_end`,
  `^KC`→`block_copy`, `^KV`→`block_move`, `^KY`→`block_delete`, `^KW`→`block_write`,
  `^KH`→`block_toggle_hidden`, `^QB`→`block_jump_begin`, `^QK`→`block_jump_end` (both
  ctrl-held and plain second-key forms per the 9B prefix convention). **Reclaims `^KC`/`^KV`**
  from their 9B interim copy/paste rows (keymap.rs:357-358) — the WordStar copy/move *is* the
  block, so this is faithful, not a loss (copy/paste remain palette-reachable + the live
  selection still works via shift-select).
- **CUA preset:** only the **promote** bridge bound — `alt-b` → `mark_block_from_selection`
  (verify free in CUA; "mark **B**lock"). The other block ops are **palette-only** in CUA.
- **clear-block:** palette only (no classic WordStar key); WordStar may also bind it.
- All ops register with a `MenuCategory` (Edit) so they appear in the palette/menu.

---

## 9. Files touched

| File | Change |
|---|---|
| `wordcartel/src/editor.rs` | `MarkedBlock` struct + `marked_block`/`pending_block_begin` fields (init None); map both + pending through `change::map_pos` in `apply` + collapse→clear; `block` field init in `Buffer::from_text` |
| `wordcartel/src/blocks_marked.rs` (new) | block command bodies: begin/end/promote/copy/move/delete/jump/toggle-hidden/clear/write — keeps editor.rs lean |
| `wordcartel/src/commands.rs` or `registry.rs` | register all `block_*` ids (Edit); handlers call the new module |
| `wordcartel/src/keymap.rs` | WORDSTAR `^K`/`^Q` block binds (reclaim `^KC`/`^KV`); CUA `alt-b`→promote |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind::WriteBlock` |
| `wordcartel/src/prompt.rs` | `PromptAction::OverwriteWriteBlock` + a `Prompt::write_block_overwrite` |
| `wordcartel/src/app.rs` | `block_write` submit routing + overwrite arm (`pending_write_block`); persist block into the session entry; render layering call |
| `wordcartel/src/render.rs` | paint `MarkedBlock` (placed path, below Selection; skip when hidden); add the **`BLK`/`BLK·hidden`** status-line segment when a block is set |
| `wordcartel/src/state.rs` | `StateEntry.block: Option<(usize,usize)>` (serde default) |
| `wordcartel/src/app.rs` (`restore_resume`) | restore `marked_block` from `entry.block` (staleness-guarded; hidden=false) |
| `wordcartel-core/src/theme.rs` | `SemanticElement::MarkedBlock` + `ThemeFaces.marked_block` + defaults across the 13 builtins / `from_base16` / `face`/`element_from_key` / a11y test lists |

---

## 10. Testing

- **edit-tracking:** block moves on an insert before it; grows on an insert inside; shrinks on
  a delete inside; **collapses → cleared** when the whole region is deleted; promote sets the
  block from the selection AND clears the selection.
- **creation:** `^KB`+`^KK` forms a normalized block; `^KK` without `^KB` → status, no block;
  promote from a non-empty selection; promote with empty selection → status; **empty block
  rejected** — `^KK` at the begin position (and zero-width promote) sets no block + status
  "empty block".
- **ops:** `block_copy` inserts at caret, **caret lands at the end of the inserted text**, and
  **keeps** the block (assert block still present + content); `block_move` produces the text at
  the caret (caret at its end), removes the original, **clears** the block, is **one undo
  step**, and is a **no-op when the caret is inside** the block; `block_delete` removes the
  range + clears; `block_jump_begin/end` move the caret + record a jump-back + unfold a folded
  target; `block_toggle_hidden` flips the flag; `block_clear` clears; every op with **no
  block** → "no marked block".
- **undo interaction:** `block_delete` then **undo** restores the text but **not** the block
  (the block is editor state, not in the undo transaction — matches marks/selection).
- **`^KW`:** writes the block text to a new path (`file::open` reads it back); existing target
  → `OverwriteWriteBlock` confirm; the **document is unchanged** (path/saved_version/swap
  untouched); block stays.
- **persistence:** a saved entry round-trips the block; `restore_resume` restores it under a
  matching mtime+size and **discards** it under a mismatch; `hidden` resets to false on reload;
  pre-9A `session.toml` (no `block` key) loads via serde default.
- **cue/render:** `MarkedBlock` paints with a face **distinct from `Selection`**; in no-color
  it carries a **modifier** (`reverse + italic`), not just color (§13.2), and the a11y
  **pairwise-distinct** test (incl. vs the diag underlines) passes with `MarkedBlock` added;
  `hidden` block not painted; overlap with selection → selection on top.
- **status indicator:** with a block set, the status line shows **`BLK`**; with `hidden` set,
  **`BLK·hidden`**; no block → no segment.
- **keymap:** WORDSTAR `^KB`/`^KK`/`^KC`/`^KV`/`^KY`/`^KW`/`^KH`/`^QB`/`^QK` resolve to the
  `block_*` ids (both prefix forms); `^KC`/`^KV` no longer resolve to copy/paste; CUA `alt-b`
  → promote; `both_presets_resolve_against_builtins` + the collision/prefix-shadow test pass.

---

## 11. Out of scope (explicitly deferred)

- **Multiple** marked blocks (single block only).
- **Column / rectangular** ("column mode", WordStar `^KN`) blocks — markdown is linear text.
- Block **indent / case-convert / reformat** ops (`^K>`/`^K<`, etc.) — markdown is
  source-as-is; formatting lives in markdown syntax + pandoc export.
- `^KP` **print** block.
- Persisting the half-set `pending_block_begin` or the `hidden` flag across sessions.
