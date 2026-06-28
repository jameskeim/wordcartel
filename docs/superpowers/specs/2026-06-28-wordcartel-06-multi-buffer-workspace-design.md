# Effort 6 — Multi-Buffer Workspace + Scratch Buffer — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-28
**Crates:** `wordcartel` (shell). No `wordcartel-core` changes anticipated beyond
incidental.

## Goal

Turn the latent multi-buffer capacity already present in the editor into a real
workspace: open several documents at once, switch between them, and add one
permanent path-less **scratch buffer** that serves as an open-ended text stash —
the successor to the "numbered blocks" idea we dropped when we settled on the
WordStar single-block model.

## Background — what already exists

Effort 7 deliberately built the open/save path buffer-extensibly. Today's
single-buffer behavior runs on top of multi-buffer-ready infrastructure:

- `Editor.buffers: Vec<Buffer>` + `active: usize` (editor.rs) — already a
  collection, currently only ever holding one member.
- `BufferId(u64)` + `Editor::alloc_id()` + `by_id()` / `by_id_mut()`.
- Async jobs (save, filter, export, clipboard paste) already carry `buffer_id`
  and merge via `by_id_mut(buffer_id)` — never `active()` — so a result lands on
  its originating buffer even after a buffer switch. (save.rs cites "multi-buffer,
  Effort 6" verbatim.)
- **Per-buffer state already lives on `Buffer`/`Document`:** selection/cursor,
  `marks`, `jump_ring`, `folds`, `marked_block`, `view.scroll`. Switching buffers
  therefore preserves each buffer's state for free.
- The clipboard `register` is global on `Editor`, so cross-buffer paste already
  works in principle.
- `open_into_current` currently *replaces* the active buffer in place
  (`buffers[active] = b`); this is the main behavior Effort 6 changes.

## Design decisions (from brainstorm)

1. **Buffer count: unbounded, managed list** (not a fixed Doc-1/Doc-2 cap). The
   workspace is first-class: visible list, quick-switch palette, easy close. Gives
   the bounded *feel* for 2–3 docs without a cap to resent later.
2. **Navigation: cycle keys + switcher palette** (both).
3. **Scratch append semantics: plain append** — scratch is an ordinary editable
   buffer; sending a block appends its text; multiplicity is "several paragraphs
   in a buffer," no special parsing.
4. **Scratch verbs: both copy-to-scratch and move-to-scratch** (mirrors `^KC` vs
   `^KV`).
5. **No cross-buffer block ops** — the global clipboard and the scratch buffer
   already move text between documents; the `marked_block` stays strictly
   within-buffer (preserves 9A's simplicity).
6. **Persistence: scratch content only** — the open-file *set* is NOT durable
   (launch opens only what you name; no surprise reopen). Per-file state stays
   per-file. Scratch content persists because it has no file backing.
7. **Quit with unsaved changes: summary with review fallback** — Save All /
   Review each / Cancel; no single-key discard-all.
8. **Open/New are additive, with throwaway reuse** — reuse the active buffer only
   if it is empty, untitled, clean, and not scratch.
9. **Cycle keys:** WordStar `^K ,` / `^K .` (prev/next, plain-only second key,
   in the `^K` block/**file** prefix); CUA `Alt+,` / `Alt+.`. (`,`/`.` land on
   `<`/`>` on QWERTY — the prev/next mnemonic — without needing Shift.)

## Components

### Scratch buffer

- A permanent, un-closeable, path-less `Buffer` named `*scratch*`, created at
  startup as a normal member of `Editor.buffers`.
- Tracked via a stored `Editor.scratch_id: BufferId` so lifecycle rules can
  special-case it (cannot close; excluded from throwaway-reuse; excluded from
  dirty/quit accounting; excluded from the "last non-scratch buffer" invariant).
- It is a full member of the buffer list: reachable via cycle, switcher palette,
  and `goto_scratch`.
- Every existing command works inside it unchanged (motion, block, fold, search,
  copy/paste).
- **Never considered "dirty"**: it has no file and is auto-persisted to session
  state, so all save/quit prompts skip it.

### Scratch verbs

- **`copy_block_to_scratch`**: append the *active* buffer's marked-block text to
  the *end* of the scratch buffer. A blank line separates entries; no leading
  blank line when scratch is currently empty. Source buffer is left intact. The
  active buffer's `marked_block` requirement matches the other block ops (no
  block → status message, no-op).
- **`move_block_to_scratch`**: same append, *and* delete the block from the
  source buffer.
- **Cross-buffer undo model (explicit):** `move_block_to_scratch` mutates two
  buffers, and history is per-buffer. It is therefore **two independent undo
  steps**: undo in the *source* buffer restores the deleted block; undo in the
  *scratch* buffer removes the appended text. There is no atomic cross-buffer
  undo — per-buffer history cannot span buffers. This is documented behavior, not
  a bug. The append to scratch and the delete from source are each a single undo
  step within their respective buffer's history.

### Navigation

- **`next_buffer` / `prev_buffer`** — step through `buffers` in **stable list
  order** (predictable; scratch is included as a member). Wraps at the ends.
- **`switch_buffer`** — opens the existing palette overlay listing open buffers in
  **MRU order** (jump-to-last-used), maintained via a small access-order list on
  `Editor` (`Vec<BufferId>` touched on every switch). Type to fuzzy-filter, Enter
  to jump. Rows show display name (`*scratch*` / `*untitled*` / filename), a dirty
  marker, in MRU order.
- **`goto_scratch`** — switch directly to the scratch buffer.
- Switching is **instant**: it changes `active` and reflows, exactly as open does
  today. No blocking, no I/O.

### Buffer lifecycle

- **Open / New are additive with throwaway reuse.** Rule: if the active buffer
  satisfies `is_empty() && path.is_none() && !dirty && id != scratch_id`, reuse it
  in place; otherwise push a new buffer and select it. This keeps the empty
  untitled launch buffer from accumulating as junk while being additive for any
  real document.
- **`close_buffer`** closes the active buffer; if it is dirty, the existing
  Save/Discard/Cancel prompt runs first. Special cases:
  - **Scratch cannot be closed** — no-op with a status message.
  - Closing the **last remaining non-scratch buffer** leaves a fresh empty
    untitled buffer. Invariant: the workspace always holds ≥1 ordinary buffer plus
    the scratch buffer.
- **Quit with unsaved changes** → a single summary prompt: **Save All / Review
  each / Cancel**.
  - Scratch is excluded from the unsaved count (auto-persisted).
  - **Save All** saves every dirty buffer; a dirty *untitled* buffer triggers its
    Save-As prompt (reuses Effort 7's `do_save_to` / Save-As machinery and the
    `pending_after_save` post-save chaining).
  - **Review each** drops into a per-buffer walk: switch-to → show → Save /
    Discard / Cancel, one dirty buffer at a time, then quit.
  - **Cancel** aborts the quit.
  - No single-key discard-all is offered.

### Keybindings

Confirmed (this design):

| Command | WordStar | CUA |
|---|---|---|
| `prev_buffer` | `^K ,` (plain-only 2nd key) | `Alt+,` |
| `next_buffer` | `^K .` (plain-only 2nd key) | `Alt+.` |

Resolved in the implementation plan (exact chords + collision resolution, gated
by the existing `both_presets_resolve_against_builtins` and
collision/prefix-shadow tests + Codex plan review):

| Command | WordStar (proposed home) | CUA (proposed home) | Dedicated? |
|---|---|---|---|
| `switch_buffer` | `^K l` (list) | `Alt`-chord | yes |
| `move_block_to_scratch` | `^K`-prefix letter | `Alt`-chord | yes |
| `copy_block_to_scratch` | `^K`-prefix letter | `Alt`-chord | yes |
| `close_buffer` | — | — | palette/menu only |
| `goto_scratch` | — | — | palette/menu only |

WordStar buffer/scratch ops live under the `^K` "block/file" prefix (consistent
with `^KW` write-block-to-file etc.). `^K ,` / `^K .` are plain-only second keys,
following the `^KM` / `^KJ` precedent (`Ctrl+,` is not terminal-deliverable). CUA
uses `Alt`-chords because bare `,`/`.` are literal text and `Ctrl+punctuation` is
unreliable across terminals. Cycle keys via `Ctrl+Tab` are explicitly avoided for
the same deliverability reason.

### Persistence

- **Per-file state** (cursor, scroll, marks, folds, marked block; keyed by
  canonical absolute path) is unchanged. Restored when that file is reopened,
  staleness-guarded by mtime+size as today.
- **The open-file set is NOT durable.** Launch opens only the file(s) named on the
  CLI / opened explicitly. No workspace-restore.
- **Scratch content IS durable.** Persisted to a new path-less slot in
  `state.toml` — a `[scratch]` section holding the scratch text and its cursor
  offset — written by `persist_session` and restored at startup (before/around
  the existing `restore_resume` flow). If no saved scratch exists, scratch starts
  empty. Restored scratch cursor is clamped/snapped to a char boundary within
  `[0, len]` (same discipline as 9A's `clamp_snap` for restored offsets).

### Status line

Add a compact buffer-position indicator (e.g. `[2/4]`) alongside the existing
filename / dirty marker / BLK / Ln:Col. Untitled and scratch buffers render as
`*untitled*` / `*scratch*` in place of a path. The existing status-line builder
already reads `editor.active()`; this adds the index/count and the display-name
fallback.

## Data flow

- **Switch** (`next`/`prev`/`switch_buffer`/`goto_scratch`): mutate `active`
  (+ update the MRU access list), reflow the now-active buffer's view. Pure
  in-memory; instant.
- **Open/New**: throwaway-reuse check → either replace active in place or
  `alloc_id()` + push + select. Existing open path (`Buffer::from_file`,
  `restore_resume`) runs for the target buffer.
- **copy/move_block_to_scratch**: read source `marked_block` range → append text
  to scratch buffer (own edit/undo) → (move only) delete range from source (own
  edit/undo).
- **Close**: dirty-prompt if needed → remove from `buffers` (never scratch) →
  select the neighbor at the same index (the buffer that shifts into the closed
  slot; if the closed buffer was last, select the new last) → enforce
  ≥1-ordinary-buffer invariant. The newly-active buffer is moved to the front of
  the MRU list.
- **Quit**: collect dirty non-scratch buffers → summary prompt → Save-All
  (chained saves, Save-As for untitled) / Review-each (per-buffer walk) / Cancel.
- **Persist**: `persist_session` writes per-file active state (as today) plus the
  `[scratch]` section. **Restore**: read `[scratch]` at startup; per-file restore
  on open unchanged.

## Error handling / edge cases

- No marked block when invoking a scratch verb → status message, no-op (matches
  existing block ops).
- Closing scratch → no-op + status; never removed from `buffers`.
- Closing the last ordinary buffer → replaced by a fresh empty untitled buffer.
- Quit Cancel at any point aborts the whole quit.
- Save-All encountering an untitled dirty buffer → its Save-As prompt; Cancel
  there aborts the quit (no data loss).
- In-flight async job for a buffer that is then closed → existing
  `by_id_mut(id) → None` no-op merge (already handled by the job routing).
- Restored scratch offset out of range / mid-char → clamp+snap (9A discipline);
  never panics a `slice()`.
- Scratch is never counted dirty; quitting with only-scratch-modified does not
  prompt.

## Testing strategy

- Per-buffer state isolation: open two buffers, set distinct cursor/marks/folds/
  block in each, switch back and forth, assert each preserved.
- Additive open with throwaway reuse: launch (empty untitled) → open file reuses
  it; open second file pushes new; reuse excludes scratch and excludes
  dirty/named buffers.
- Close rules: scratch close is no-op+status; closing last ordinary buffer yields
  a fresh untitled; dirty close prompts.
- Quit: Save-All saves all dirty (incl. Save-As for untitled); Review-each walks
  per buffer; Cancel aborts; scratch excluded from count.
- Scratch append: empty vs non-empty (separator logic); copy leaves source; move
  deletes source.
- move_block_to_scratch two-buffer mutation: source shortened, scratch grown;
  undo in source restores; undo in scratch removes; independence verified.
- Scratch persistence round-trip: write text, persist, restore at startup;
  out-of-range restored offset clamps (no slice panic).
- Navigation: cycle wraps in stable order incl. scratch; switcher lists MRU
  order; goto_scratch jumps.
- Keymap: both presets resolve `prev_buffer`/`next_buffer` (and the
  plan-assigned chords) against builtins with no collision / prefix-shadow.
- Status line: `[i/n]` indicator; `*scratch*` / `*untitled*` display names.

## Out of scope (explicitly deferred)

- Window/pane **splits** (multiple views of one document) — a later effort;
  Document/View stays fused for now, but the design does not foreclose splits.
- **Workspace/session restore** of the open-file set (Q6 chose scratch-only).
- **Cross-buffer block** operations (clipboard + scratch cover it).
- **SSH/tmux clipboard fix** — tracked as its own future effort (OSC 52
  passthrough + bracketed paste); orthogonal to multi-buffer.
- Structured scratch entry markers / named registers (plain append chosen).
