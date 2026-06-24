# Wordcartel Multi-Buffer Workspace — Design Spec

**Date:** 2026-06-24
**Status:** Design approved (brainstorm) — pending spec review → plans
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§5 Backlog "Multiple buffers / windows"; §10.2 flat `Editor`)
**Predecessors:** Effort 4a (sync shell), 4b-1 (async substrate), 4b-2 (crash safety) — all merged.
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

---

## 1. Goal

Let the user hold several open documents at once and switch between them, **without a
core refactor and without regressing the single-document experience.** The parent
spec lists "Multiple buffers / windows" as **post-v1 backlog**; this design splits
that into (a) a small **behavior-preserving prep refactor** done now (the substrate),
and (b) a **post-1.0 feature effort** that adds the multiplicity, mirroring how the
plugin system (§18) carries substrate requirements onto earlier efforts so it can
land later cleanly.

## 2. Scope (decided)

- **In:** N open buffers, **one visible at a time**, switchable (vim `:bn`/`:bp`
  style). Open / close / switch. Per-buffer crash safety. Multi-buffer quit.
- **In (substrate only, now):** the `Buffer`/`BufferId` extraction + job-routing-by-id,
  done as a behavior-preserving refactor so later efforts build on it.
- **Out (this effort), designed-for-later:** **split panes / windows** (multiple
  buffers visible at once). The buffer layer is shaped so a future window-tree can
  reference `BufferId`s and tile per-buffer `View`s additively — but no geometry,
  focus-tree, or resize work is in scope here.
- **Out:** tabs-as-chrome UI beyond a one-line status indicator (the ledger files
  `tui-tabs` as out-of-scope for 1.0); workspace/session persistence across runs.

## 3. Architecture

### 3.1 `Buffer` is the unit; `Editor` is a thin workspace over a vec of them

Today `Editor` is a flat single-document struct that **conflates one document's
transient state with global app state**. The refactor separates the two.

```rust
pub struct Editor {
    pub buffers: Vec<Buffer>,   // invariant: len >= 1 (never empty)
    pub active: usize,          // index into `buffers` of the focused buffer
    // --- global app state (shared across buffers) ---
    pub register: Register,     // clipboard: copy in A, paste in B
    pub status: String,         // one status line
    pub prompt: Option<Prompt>, // one modal at a time
    pub quit: bool,
    pub pending_swap_body: Option<String>, // recovery-on-open staging (startup)
    pub pending_swap_path: Option<PathBuf>,
}

pub struct Buffer {
    pub id: BufferId,           // stable, monotonic; NOT the vec index
    pub document: Document,     // buffer, selection, history, version, saved_version, path, stored_fp, blocks
    pub view: View,             // scroll, scroll_row, area, mode, line_layouts (already per-document)
    // --- per-buffer transient state, RELOCATED off Editor ---
    pub desired_col: Option<usize>,
    pub pre_edit_rope: Option<ropey::Rope>,
    pub last_edit: Option<block_tree::Edit>,
    pub last_edit_at: Option<u64>,
    pub last_swap_at: Option<u64>,
    pub swap_in_flight: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct BufferId(pub u64);
```

Accessors: `editor.active() -> &Buffer`, `editor.active_mut() -> &mut Buffer`,
`editor.by_id(id) -> Option<&Buffer>` / `by_id_mut`. The `active`/`active_mut`
helpers assert `!buffers.is_empty()` (the len≥1 invariant).

**`BufferId` is a stable monotonic `u64`**, assigned from an `Editor`-held counter at
buffer creation — **not** the `Vec` index, because indices shift when a middle buffer
is closed. Job results and (future) window panes reference buffers by `id`.

**Why this shape (vs. alternatives considered):**
- *Parallel vecs* (`Vec<Document>` + `Vec<View>`): rejected — desync-prone.
- *Flat `Editor` + swap document/view in-and-out on switch*: rejected — switching
  would have to copy transient state in/out; error-prone.
- *Bundled `Buffer` + stable `BufferId`*: chosen — one cohesive unit; the boundary
  that makes a future split-pane layer additive (a window-tree maps geometry →
  `BufferId`; `View` already per-buffer means tiling N views needs no document
  changes).

### 3.2 Global vs. per-buffer (the boundary)

- **Global (on `Editor`):** clipboard `register` (cross-buffer copy/paste), the single
  `status` line, the single active `prompt` modal, the `quit` flag, and recovery-open
  staging.
- **Per-buffer (on `Buffer`):** everything tied to one document's text, history,
  caret/selection, layout/scroll, render mode, and async/cadence/derive transients.

This boundary is the single most important design decision; getting it right is what
keeps switching from cross-contaminating caret column, swap cadence, or derive hints.

### 3.3 Job routing by `BufferId` (the one real interaction with 4b)

4b's async substrate dispatches `Save`/`SwapWrite` jobs whose `merge` runs on the
foreground. With one document the merge implicitly targets it. With N buffers, a
result must merge into the buffer it was dispatched **for** — even if the user has
since switched away or closed it.

- `Job` and `JobResult` gain `pub buffer_id: BufferId`.
- Staleness key becomes `(buffer_id, version)`: `is_stale` first resolves the buffer
  by id; **if the buffer was closed, the result is dropped** (a no-op merge); else the
  existing per-kind staleness (`Save`/`SwapWrite` never stale; coalescible discard on
  version change) applies **within that buffer**.
- `apply_result` looks up `editor.by_id_mut(result.buffer_id)`; if `None` (closed),
  drop; else run the merge against that buffer.
- The save/swap `merge` closures, which today touch `editor.document.*` /
  `editor.last_swap_at`, instead receive (or capture) the target `&mut Buffer`.

**Prep timing:** the `buffer_id` field is added during the prep refactor and is
**inert with one buffer** (always resolves to the single buffer). This lets Effort 4c
(filters dispatch jobs) and Effort 5 (spellcheck/search dispatch jobs) be
buffer-routable for free, instead of being reworked at Effort 6.

## 4. Lifecycle & UX

| Concern | Behavior |
|---|---|
| **CLI** | `wcartel a.md b.md c.md` opens all three as buffers, `active = 0`. `wcartel` alone → one scratch buffer. Each path resolved/opened with the existing 4a open logic + recovery-on-open per buffer. |
| **Open into new** | An `open <path>` command opens the file into a **new** buffer (does not replace the active one) and switches to it. Reopening an **already-open path** switches to its existing buffer (dedupe by canonicalized realpath) — never two buffers over one file (which would fight over one swap). |
| **Switch** | `next_buffer` / `prev_buffer` cycle `active` (wrapping); on switch, **re-derive the newly-active buffer's view** (`derive::rebuild` + `ensure_visible`) so its layout/caret is current. Default keys deferred to Effort 5 keymap (proposed `Ctrl+PageDown`/`Ctrl+PageUp`). A palette-backed picker (Effort 5) lists buffers with name + dirty marker. |
| **Close** | `close_buffer` closes the active buffer. If it is **dirty**, raise a **close-confirm modal** — `[S]ave & close · [C]lose anyway · [Esc] cancel` — reusing 4b-2's `prompt` infra (a new `PromptAction::{SaveAndClose, CloseAnyway}` + a `Prompt::close_confirm()` constructor). On clean close, delete that buffer's swap (version-aware, as today). |
| **Close last** | Closing the **last** buffer leaves a **fresh empty scratch buffer** (the app never ends via close). Quit is the only path that exits. |
| **Status indicator** | The status line shows `[<active+1>/<count>] <name><dirty> [<mode>] <status>` — e.g. `[2/3] notes.md* [live] Saved`. With a single buffer the `[1/1]` prefix MAY be omitted to keep the current look. |
| **Quit (multi-dirty)** | `Ctrl+Q` with **any** dirty buffer → quit-confirm reporting the count: `N unsaved buffers — [S]ave all & quit · [Q]uit anyway · [C]ancel`. "Save all" dispatches a save per dirty buffer and bounded-joins (generalizing 4b-2's `quit_after_save` to a set / count of awaited `(buffer_id, version)` results). Clean buffers' swaps are deleted on exit. |
| **Per-buffer crash safety** | Each buffer keeps its **own** swap. Named buffers: path-hash-keyed (unchanged). **Scratch buffers re-keyed `scratch-<pid>-<bufferid>.swp`** — today's `scratch-<pid>.swp` collides once there is more than one scratch buffer in a process. Recovery-on-open assesses each **named** buffer's swap (hash-first predicate, unchanged) and enumerates **scratch orphans** from dead PIDs (the 4b-2 orphan finder, extended to ignore the per-buffer suffix). The panic dump already writes one file per snapshot; it dumps the **active** buffer's last-good snapshot (a future refinement could dump all dirty buffers). |

## 5. Where it lands in the plan

| | Effort | When | Depends on |
|---|---|---|---|
| **Substrate** | **Buffer-extraction refactor** (behavior-preserving) | **Before Effort 4c** | 4b (merged) |
| **Feature** | **Effort 6 — Multi-buffer workspace** (post-1.0) | **After Effort 5** (for the palette picker only) | prep refactor + 4b; one task gated on Effort 5 |

The prep refactor is the substrate-requirement, paid down now while the code is fresh
and before 4c/5 add more `editor.document.*` call sites and more per-document state
(filters, spellcheck, incremental search). The feature effort is post-1.0; all of it
except the palette-backed picker needs only the prep + 4b, so the bulk could land
before Effort 5 if priorities shift (cycle-switching works without a palette).

## 6. Task breakdown

### 6.1 Prep refactor — "Buffer extraction" (before 4c)

Behavior-preserving; **all current shell tests (136 at time of writing) stay green**;
the running binary behaves identically.

1. **Introduce `Buffer` + `BufferId` + the new `Editor` shape** (vec-of-one,
   `active = 0`); `active()`/`active_mut()`/`by_id`/`by_id_mut` accessors;
   `new_from_text` builds exactly one buffer. The `Document`/`View` structs are
   unchanged — only their ownership moves into `Buffer`.
2. **Migrate shell call sites** `editor.document.*` / `editor.view.*` /
   `editor.<transient>` → through `active()`/`active_mut()`. Split across files to
   keep each change reviewable: (a) `editor.rs` + `derive.rs` + `nav.rs`;
   (b) `commands.rs` + `render.rs`; (c) `save.rs` + `swap.rs` + `app.rs`. ~2 tasks.
3. **Thread `buffer_id` through the job model:** add `buffer_id` to `Job`/`JobResult`;
   `apply_result` routes via `by_id_mut` (drop if closed); `is_stale` keyed on
   `(buffer_id, version)`; save/swap merges target the resolved `&mut Buffer`. Inert
   with one buffer.
4. **Verify:** full workspace suite + 3× parallel + a manual binary smoke (open, edit,
   save, swap cadence, recovery, panic dump all behave exactly as before).

→ ~5 tasks; small, low-risk, gated entirely by the existing test suite.

### 6.2 Effort 6 — Multi-buffer workspace (post-1.0)

1. **N buffers + CLI multi-file open** (`wcartel a b c`, active=0; per-buffer
   recovery-on-open at startup).
2. **Switch commands** `next_buffer`/`prev_buffer` + re-derive on switch + the
   `[i/n] name*` status indicator.
3. **Open-into-new** command + open-path dedupe (reopen → switch to existing).
4. **Close** command + close-with-dirty modal (`SaveAndClose`/`CloseAnyway`) +
   close-last → fresh scratch.
5. **Per-buffer scratch-swap re-keying** (`scratch-<pid>-<bufferid>.swp`) +
   multi-buffer recovery-on-open + scratch-orphan enumeration extended.
6. **Quit with multiple dirty buffers** (save-all & quit; await the set of
   `(buffer_id, version)` save results, bounded).
7. **Job-routing-by-buffer-id made real:** integration test that a save/swap
   completing for a non-active or closed buffer merges into the right buffer / no-ops.
8. **Palette-backed buffer switcher** (depends on Effort 5 palette) — picker UI with
   name + dirty.
9. **Final review + crash-safety integration tests** (multi-buffer recovery, multi-dirty
   quit, swap isolation across buffers).

→ ~8–9 tasks. Only task 8 is gated on Effort 5.

## 7. Testing strategy

- **Prep refactor:** the existing suite is the gate — no behavior change permitted.
  Add focused unit tests for the new accessors (`active`/`by_id` invariants) and for
  job routing by id (with one buffer, a result for that id merges; a result for a
  bogus id no-ops).
- **Effort 6:** per-buffer isolation (edit/save/swap in buffer A leaves buffer B
  untouched); switch re-derive (B's caret/scroll restored on switch back); close-dirty
  modal matrix; close-last→scratch; open-path dedupe; multi-buffer recovery-on-open;
  multi-dirty save-all-&-quit; **job routes to the correct buffer after a switch and
  no-ops after a close** (the §3.3 invariant). Determinism via `InlineExecutor` +
  injected `Clock`, as in 4b. Any swap-writing test uses unique temp paths (the 4b-2
  parallel-isolation discipline).

## 8. Non-goals (explicit)

- **Split panes / windows** (multiple buffers visible at once) — designed-for, not
  implemented; a later sub-effort wraps buffers in a window-tree.
- **Session/workspace persistence** across runs (reopen the same set of buffers).
- **Tab-bar chrome** beyond the one-line status indicator.
- **Per-buffer independent render modes shown simultaneously** (only the active
  buffer renders; its own `view.mode` already persists per buffer).
- **Dumping all dirty buffers on panic** (the panic dump stays single-snapshot for
  now; noted as a future refinement).

## 9. Spec → parent-section traceability

| This spec | Parent § | Item |
|---|---|---|
| §2, §8 | 5 Backlog | "Multiple buffers / windows" — post-v1, now split prep + feature |
| §3.1, §3.2 | 10.2 | flat `Editor` (Document/View split kept) → `Editor` over `Vec<Buffer>` |
| §3.3 | 10.3, 4b | async edges over snapshots; job staleness now `(buffer_id, version)` |
| §4 close/quit | 15.2, 4b-2 | modal only for destructive decisions; reuse swap/recovery + modal infra |
| §4 crash safety | 15.7, 4b-2 | per-buffer swap/recovery; scratch re-keying; orphan enumeration |
| §5 | 5, 18 | post-1.0 effort carrying a substrate requirement onto earlier efforts |
