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
- **Save, filter, and clipboard-paste** results already carry `buffer_id` and
  merge via `by_id_mut(buffer_id)` — never `active()` — so they land on the
  originating buffer even after a switch (save.rs:61; filter app.rs:191;
  clipboard app.rs:659). (save.rs cites "multi-buffer, Effort 6" verbatim.)
- **Export is the exception** (Codex I1): it captures `buffer_id` (export.rs:98)
  but the reducer discards it — `apply_export_done` writes a file path and sets
  status, with no buffer merge (app.rs:232, 1608). That's fine: export targets a
  *file*, not a buffer, so it needs no per-buffer routing. Effort 6 leaves export
  as-is; the only open question is whether the export *overwrite prompt* should be
  bound to its originating buffer — decided **no** for now (the prompt is modal and
  resolved before any switch matters; revisit only if export becomes async-and-
  switchable).
- **Filter/transform are global single-flight** (Codex I2): `filter_in_flight` /
  `transform_in_flight` are single flags on `Editor` (editor.rs:288) and a second
  job is rejected workspace-wide (filter.rs:327; app.rs:178 already notes per-buffer
  tracking would be needed for concurrency). **Effort 6 deliberately keeps these
  single-flight across the whole workspace** — no per-buffer job maps. (YAGNI; a
  filter is a brief modal-ish operation. Per-buffer concurrent jobs are out of
  scope.)
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
- **`Editor.scratch_id: BufferId` is a NEW field** (Codex I6 — it does not exist
  today: editor.rs:271 has `buffers` / `active` / `next_buffer_id` only). Lifecycle
  rules special-case scratch by id: cannot close; excluded from throwaway-reuse;
  excluded from dirty/quit accounting; excluded from the "last non-scratch buffer"
  invariant.
- **Startup change** (Codex I6): today init creates one buffer and replaces
  `buffers[0]` with the CLI file (app.rs:1725), and a test pins
  `buffers.len() == 1` (editor.rs:683). Effort 6 init instead creates the scratch
  buffer **plus** one ordinary buffer (the CLI file, or an empty untitled buffer);
  `scratch_id` is recorded; the old slot-0 replacement becomes the normal
  throwaway-reuse path. The single-buffer test (editor.rs:683) is updated to the
  new startup invariant (one ordinary buffer + scratch).
- It is a full member of the buffer list: reachable via cycle, switcher palette,
  and `goto_scratch`.
- Every existing command works inside it unchanged (motion, block, fold, search,
  copy/paste).
- **Never considered "dirty" — via a new predicate** (Codex I3): dirty is computed
  purely as `Some(version) != saved_version` (editor.rs:53), and `Buffer::apply`
  bumps `document.version` on every edit (editor.rs:167) — so an edited scratch
  buffer *would* read as dirty under the raw predicate. Effort 6 adds
  **`Editor::is_dirty(buffer_id) -> bool`** that returns `false` for `scratch_id`
  and otherwise applies the raw predicate. All dirty-sensitive sites route through
  it: the status-line dirty marker, quit accounting, the close prompt, the
  throwaway-reuse check, and **swap-eligibility** (app.rs:1619 treats a dirty
  active buffer as swap-eligible — scratch is path-less so the swap/persist path
  already early-returns on `path == None` at app.rs:2023, but routing through
  `is_dirty` makes the exclusion explicit rather than incidental).

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
  - Scratch is excluded from the unsaved count (via `is_dirty`, auto-persisted).
  - **New multi-buffer quit-save state machine** (Codex I5): the existing
    `Editor.pending_after_save` is a *single* `Option<PendingAfterSave>`
    (editor.rs:280) and the current quit prompt only offers Save & quit / Quit
    anyway / Cancel (prompt.rs:50) — neither can sequence N saves. Effort 6 adds a
    quit-save driver on `Editor`: a **queue of dirty `BufferId`s** plus a `mode`
    (SaveAll | ReviewEach) and a current Save-As target. It processes the queue one
    buffer at a time, reusing Effort 7's per-buffer `do_save_to` / Save-As prompt
    for each; `editor.quit` is set only when the queue **drains**. Any Cancel
    (including Cancel inside an untitled buffer's Save-As) **aborts the whole quit**
    and clears the queue — no data loss.
  - **Save All** enqueues every dirty (non-scratch) buffer and drains the queue;
    a dirty *untitled* buffer in the queue raises its Save-As prompt.
  - **Review each** drains the same queue but, per buffer, switches-to → shows →
    asks Save / Discard / Cancel before advancing.
  - **Cancel** aborts the quit and clears the queue.
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
- **Scratch content IS durable** (schema corrected per Codex I4). `SessionState`
  today serializes *only* `entries: BTreeMap<String, StateEntry>` (state.rs:37), so
  a bare `[scratch]` key cannot just be dropped in. Effort 6 adds a **typed sibling
  field** `scratch: Option<ScratchState>` to `SessionState` (kept *outside*
  `entries`; `ScratchState { text: String, cursor: usize }`, `#[serde(default)]`).
  Because it is a named struct field, it serializes as its own `[scratch]` table
  with no key collision against the `[entries."/path"]` map.
- **Persist must not piggyback on the active buffer.** `persist_session` currently
  early-returns when the active buffer's `path == None` (app.rs:2023) and is
  triggered off the active buffer's `saved_version` (app.rs:1967). Scratch is
  path-less and frequently the *inactive* buffer, so its persistence is written
  **explicitly and unconditionally** (whenever the workspace persists), independent
  of which buffer is active and independent of the path==None early-return.
- If no saved scratch exists, scratch starts empty. The restored scratch cursor is
  clamped/snapped to a char boundary within `[0, len]` (same discipline as 9A's
  `clamp_snap` for restored offsets) so a stale offset never panics a `slice()`.

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
  out-of-range restored offset clamps (no slice panic); `SessionState.scratch`
  serializes as a `[scratch]` table without colliding with `[entries."/path"]`.
- `Editor::is_dirty`: returns false for an edited scratch buffer; true for an
  edited ordinary buffer; status marker / quit count / close prompt / throwaway
  reuse / swap-eligibility all honor it.
- Quit-save state machine: Save-All drains a multi-buffer queue (incl. Save-As for
  an untitled member); Review-each walks per buffer; Cancel mid-queue aborts the
  whole quit and clears the queue (no buffer left half-processed).
- Navigation: cycle wraps in stable order incl. scratch; switcher lists MRU
  order; goto_scratch jumps.
- Keymap (Codex M1, explicit): add parse tests for the chord strings
  `"ctrl-k ,"`, `"ctrl-k ."`, `"alt-,"`, `"alt-."` (the parser accepts any
  single-char token, keymap.rs:99, but these specific tokens are not yet
  test-pinned); and no-collision / no-prefix-shadow checks for the new commands in
  both presets after registration (extending the existing
  `both_presets_resolve_against_builtins` + WordStar prefix-shadow tests at
  keymap.rs:713). Note CUA currently binds `ctrl-.` (keymap.rs:297) but not
  `alt-,`/`alt-.`, and the WordStar `^K` subtree binds letters/digits but not
  comma/period — both new homes are free.
- Status line: `[i/n]` indicator; `*scratch*` / `*untitled*` display names.

## New code surface (checklist for the plan; from Codex review)

- `Editor.scratch_id: BufferId` field + scratch creation at init; remove the
  startup `buffers[0]` replacement (app.rs:1725); update the single-buffer test
  (editor.rs:683). [I6]
- `Editor::is_dirty(BufferId) -> bool` (scratch-aware); route status marker, quit
  count, close prompt, throwaway-reuse, swap-eligibility through it. [I3]
- `SessionState.scratch: Option<ScratchState>` sibling field + explicit,
  active-buffer-independent scratch persist/restore. [I4]
- Quit-save state machine (queue of dirty `BufferId`s + SaveAll/ReviewEach mode +
  Save-As target; quit only on drain; Cancel aborts). [I5]
- `next_buffer`/`prev_buffer`/`switch_buffer`/`goto_scratch`/`close_buffer`/
  `copy_block_to_scratch`/`move_block_to_scratch` commands + MRU access list on
  `Editor`; keymap chords + parse/collision tests. [M1]
- Export routing left as-is (file-targeted, no buffer merge); filter/transform
  kept workspace-single-flight. [I1, I2]

## Out of scope (explicitly deferred)

- Window/pane **splits** (multiple views of one document) — a later effort;
  Document/View stays fused for now, but the design does not foreclose splits.
- **Workspace/session restore** of the open-file set (Q6 chose scratch-only).
- **Cross-buffer block** operations (clipboard + scratch cover it).
- **SSH/tmux clipboard fix** — tracked as its own future effort (OSC 52
  passthrough + bracketed paste); orthogonal to multi-buffer.
- Structured scratch entry markers / named registers (plain append chosen).
