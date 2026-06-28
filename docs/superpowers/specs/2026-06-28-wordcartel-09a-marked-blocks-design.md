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
  (editor.rs:162). **Boundary semantics (Codex):** map `start` via **`map_pos`** and `end`
  via **`map_pos_before`** — so an insert exactly at a boundary stays *outside* the block
  (insert-at-start lands before the new start; insert-at-end lands after the unchanged end),
  giving a clean half-open `[start, end)` that **grows only on a strictly-interior insert**
  and shrinks on an interior delete; the block **moves** on an external insert/delete.
- **Collapse → clear:** after mapping, if `start == end` (the region was fully deleted) →
  `marked_block = None`. A half-set `pending_block_begin` that maps onto a deletion just
  tracks; it is dropped on completion/replacement.
- **Undo/redo (Codex correction):** `Editor::undo`/`redo` call `history.undo(&mut buffer)`
  **directly — they bypass `apply`**, so they do NOT map `marks`/`jump_ring`/the block through
  the inverse changeset. To avoid acting on **stale** byte offsets (a block whose endpoints no
  longer match the reverted text could copy/move the wrong bytes), **undo and redo CLEAR the
  marked block + `pending_block_begin`**. (Conservative, safe v1 choice — a future refinement
  could map the block through the inverse changeset instead. This is stricter than `marks`,
  which today go stale on undo; the block clears because *acting* on a stale block is more
  damaging than a stale jump.)

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

**Move ordering note (Codex — feasible via the existing API):** `block_move` reuses
`commands::build_multi_replace(edits, doc_len)`, which already composes **ascending,
non-overlapping** replacements into one `ChangeSet` + one `block_tree::Edit` (one undo step).
- caret **before** the block: edits `[(caret, caret, text), (start, end, "")]`; resulting
  caret = `caret + text.len()`.
- caret **at/after** `end`: edits `[(start, end, ""), (caret, caret, text)]`; resulting caret
  = `caret` (the deletion shifts the later insert into place).
- caret **inside** `[start, end)` → no-op + status (would overlap the edits anyway).
The contract: the block text ends up at the caret, the original is gone, the caret lands at
the **end of the moved text**, and it is **one** undo step.

**Undo interaction:** see §2 — `undo`/`redo` bypass `apply` and therefore **clear** the marked
block (they can't map it through the inverse changeset, and acting on stale offsets is unsafe).
So `^KY` then **undo** restores the text but leaves **no** block (you'd re-mark). `^KC` then
undo removes the inserted copy and clears the block.

**No system-clipboard sync (deliberate):** `^KC`/`^KV` are the block's **own** channel — they
insert directly at the caret and do **not** populate the OS clipboard / OSC-52 register. A
user who wants the system clipboard uses the live selection + `copy`. (Intentional scope
choice, stated so it is not read as an omission.)

---

## 5. `^KW` write block → file

`block_write` reuses Effort 7's Save-As minibuffer infrastructure:
- A **`MinibufferKind::WriteBlock`** prompt ("Write block to: ", pre-filled with the doc's
  dir / cwd) → resolve path (`~`/relative expansion) → if the target exists, an **overwrite
  confirm** (a new `PromptAction::OverwriteWriteBlock` + `pending_write_block:
  Option<PathBuf>`, mirroring `OverwriteSaveAs`/`pending_save_overwrite`) → `file::save_atomic`
  of the **block text** (`slice(start..end)`).
- **Factor the path expansion (Codex):** Effort 7's `save_as_submit` expands `~`/relative
  inline; extract that into a shared `expand_path(text) -> PathBuf` helper that both Save-As
  and `block_write` call (no duplication).
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
- **Persist:** the `StateEntry` is built in **`app.rs::persist_session`** (called after save
  completion and on exit) — record `block = marked_block.map(|b| (b.start, b.end))` there,
  alongside cursor/marks/folds. **Compile churn (Codex):** several tests build `StateEntry { …
  }` literals; adding the field requires `block: None` in each (or `..Default::default()`),
  exactly as the `folds` field addition did.
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
- **Monochrome-safe (§13.2) — `reverse + bold + underline` (Codex-corrected):** the block's
  mono modifier MUST be **pairwise-distinct** from every other cued element under the theming
  a11y proof. The real `mono_faces()` already occupies the **entire** 2-modifier space of the
  prominent attributes: `selection = reverse+underline`, `front_matter = reverse+italic`,
  `search_current = bold+reverse`, `diag_spelling = bold+underline`, `diag_grammar =
  italic+underline`, `strong_emphasis = bold+italic` (and the singles: emphasis=italic,
  strong/heading=bold, link=underline, code/search_match=reverse). So no 2-modifier combo of
  {bold,italic,underline,reverse} is free. **`MarkedBlock` therefore uses the 3-modifier
  `reverse + bold + underline`** — a strong, block-like highlight, distinct from `selection`
  (reverse+underline, no bold), `search_current` (bold+reverse, no underline), and
  `diag_spelling` (bold+underline, no reverse). In color: a tinted background **plus** these
  modifiers (so the cue survives at `Depth::None`). `MarkedBlock` MUST be added to the a11y
  pairwise-distinct test set, which will confirm the choice.
- **Render layering:** the block paints as a **backdrop below** Selection/Search/Diag (active
  things win where they overlap), via the same placed-path compose layering the theming work
  established (base → MarkedBlock → Selection → Search → Diag). **Forces the placed path
  (Codex):** like a visible selection, a row that intersects a non-hidden block must set
  `use_placed` so the per-cell faces are composed. `hidden == true` → not painted. **Folds
  (Codex):** a block spanning folded text is only partially visible — paint only the visible
  cells (must not panic on hidden lines); `^QB`/`^QK` unfold to the target. Overlap with the
  selection is normally absent (promote clears the selection).
- **Status-bar presence indicator.** A marked block can be entirely **off-screen**, with no
  on-screen signal that one is armed. So when `marked_block` is set, the status line shows a
  small **`BLK`** segment (themed via `SemanticElement::Chrome`). **It must NOT be gated on the
  word-count toggle (Codex):** `Ln, Col` and word-count ride the optional *right* segment
  (`word_count_segment` → `Some`), which a user can disable. The `BLK` indicator must always
  show when a block exists, so it lives in the **left** status text (path · dirty · mode · …)
  or a dedicated builder independent of word-count. When the block is **hidden** (`^KH`), it
  reads **`BLK·hidden`** (the block still exists and acts). No block → no segment.

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
| `wordcartel-core/src/theme.rs` | `SemanticElement::MarkedBlock` + `ThemeFaces.marked_block`; wire it through **every** touchpoint (Codex-enumerated): `face()`, `face_mut()`, `default()`, `tokyo_night()`, `from_base16()`, **`mono_faces()` = `reverse+bold+underline`**, `phosphor`/HSL builtins, `element_from_key()`, `ALL_ELEMENTS`, and the a11y pairwise-distinct test list |

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
- **undo interaction:** `block_delete` then **undo** restores the text and leaves **no** block
  (undo/redo bypass `apply` → the block is cleared, not mapped); a block set, then an
  unrelated `undo`, is also cleared (the conservative safe behavior).
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
  `block_*` ids (both prefix forms); `^KC`/`^KV` **no longer resolve to copy/paste** (update
  the 9B `wordstar_new_chords_resolve` assertions that expect `^KC`/`^KV`); CUA `alt-b` →
  `mark_block_from_selection`; the `block_*` ids must be **registered** so
  `both_presets_resolve_against_builtins` + the collision/prefix-shadow test still pass.

---

## 11. Out of scope (explicitly deferred)

- **Multiple** marked blocks (single block only).
- **Column / rectangular** ("column mode", WordStar `^KN`) blocks — markdown is linear text.
- Block **indent / case-convert / reformat** ops (`^K>`/`^K<`, etc.) — markdown is
  source-as-is; formatting lives in markdown syntax + pandoc export.
- `^KP` **print** block.
- Persisting the half-set `pending_block_begin` or the `hidden` flag across sessions.
