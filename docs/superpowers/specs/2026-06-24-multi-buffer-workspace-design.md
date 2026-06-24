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
    pub buffers: Vec<Buffer>,    // invariant: len >= 1 (never empty)
    pub active: usize,           // index into `buffers` of the focused buffer
    pub next_buffer_id: u64,     // monotonic id source; NEVER reused for the process lifetime
    // --- global app state (shared across buffers) ---
    pub register: Register,      // clipboard: copy in A, paste in B
    pub status: String,          // one status line (routing rules in §4)
    pub prompt: Option<Prompt>,  // one modal at a time; carries its target BufferId (below)
    pub quit: bool,
}

pub struct Buffer {
    pub id: BufferId,           // stable, monotonic; NOT the vec index; never reused
    pub document: Document,     // buffer, selection, history, version, saved_version, path, stored_fp, blocks
    pub view: View,             // scroll, scroll_row, area, mode, line_layouts (already per-document)
    // --- per-buffer transient state, RELOCATED off Editor ---
    pub desired_col: Option<usize>,
    pub pre_edit_rope: Option<ropey::Rope>,
    pub last_edit: Option<block_tree::Edit>,
    pub last_edit_at: Option<u64>,
    pub last_swap_at: Option<u64>,
    pub swap_in_flight: bool,
    // --- per-buffer recovery-on-open staging (was wrongly global; Codex review) ---
    pub pending_recovery: Option<PendingRecovery>,
}

/// A discovered swap awaiting the user's recover/discard/open-original decision
/// for THIS buffer. Per-buffer because startup can discover several at once.
pub struct PendingRecovery {
    pub swap_path: PathBuf,     // the actual swap file (orphan path differs from this buffer's own)
    pub body: String,           // swap contents, loaded on Recover
    // header fields (version, ts) available if the resolver needs them
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct BufferId(pub u64);
```

**`BufferId` allocation:** `Editor::alloc_id()` returns `BufferId(self.next_buffer_id)` and increments the counter. Ids are **never reused** for the process lifetime, so a stale in-flight job result can never collide with a newly-opened buffer (a recycled `Vec` index could).

**The active modal carries its target.** `Prompt` gains a `target: Option<BufferId>` (None for app-global prompts like multi-dirty quit; `Some` for buffer-scoped prompts like recovery and close-confirm). Every `PromptAction` resolution acts on `target`, never on `active()` — see the cross-cutting rule in §3.4.

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
  `status` line, the single active `prompt` modal (which **carries its target
  `BufferId`**), the `quit` flag, and the `next_buffer_id` counter.
- **Per-buffer (on `Buffer`):** everything tied to one document's text, history,
  caret/selection, layout/scroll, render mode, async/cadence/derive transients, **and
  its `pending_recovery` staging** (a startup discovers swaps for several buffers at
  once, so a single global recovery slot — the original draft — would let later
  candidates overwrite earlier ones; Codex review).

This boundary is the single most important design decision; getting it right is what
keeps switching from cross-contaminating caret column, swap cadence, derive hints, or
recovery prompts.

### 3.3 Job routing by `BufferId` (the one real interaction with 4b)

4b's async substrate dispatches `Save`/`SwapWrite` jobs whose `merge` runs on the
foreground. With one document the merge implicitly targets it. With N buffers, a
result must merge into the buffer it was dispatched **for** — even if the user has
since switched away or closed it.

- `Job` and `JobResult` gain `pub buffer_id: BufferId`.
- Staleness key becomes `(buffer_id, version)`: `is_stale` first resolves the buffer
  by id; **if the buffer was closed, a buffer-local-merge result is dropped** (a no-op
  merge); else the existing per-kind staleness (`Save`/`SwapWrite` never stale;
  coalescible discard on version change) applies **within that buffer**. The drop rule
  applies **only to buffer-local merges**, never to durability completions (§3.4 rule 2).
- `apply_result` looks up `editor.by_id_mut(result.buffer_id)`; if `None` (closed) and
  the result is a buffer-local merge, drop; else run the merge against that buffer.
- The save/swap `merge` closures, which today touch `editor.document.*` /
  `editor.last_swap_at`, instead receive (or capture) the target `&mut Buffer`.
- **File identity is immutable for this effort:** a buffer's `path` does not change
  while a job is in flight (save-as is Effort 5; recovery loads into the existing
  buffer without changing its path). So `(buffer_id, version)` is a sufficient key — no
  path-era token is needed now. Save-as, when it lands, must add path identity to the
  job payload.

**Prep timing & acceptance (Codex review — the "inert" trap):** the `buffer_id` field
is added during the prep refactor. With one buffer it is *inert in effect* but must be
*mechanical in code*: every job creation, staleness check, merge, and status dispatch
**must use `result.buffer_id` / `by_id_mut`, never `active()`** — otherwise a routing
bug stays invisible until N>1. The prep's acceptance criteria include a **debug
assertion** that a result's `buffer_id` resolves to the intended buffer (not merely the
active one). This is what lets Effort 4c (filters) and Effort 5 (spellcheck/search)
dispatch buffer-routable jobs for free.

### 3.4 Cross-cutting invariants (Codex review)

Two rules bind every later flow; the implementation plans must enforce them:

1. **Every deferred action and async result carries its `BufferId`** (plus the
   document `version`, and any external identity it needs such as a swap/file path).
   *No action created while observing one buffer may silently resolve to `active()` at
   execution time.* This covers job results, the active modal's actions, the
   bounded-join set for save-all-&-quit, and recovery staging.

2. **Two result classes — buffer-local merges vs. workspace durability completions.**
   - *Buffer-local merge* (status/`saved_version`/`stored_fp`/cadence bookkeeping,
     coalescible document edits): needs a live `&mut Buffer`; **dropped if the buffer
     was closed** (§3.3).
   - *Durability completion* (an atomic save's final write, a swap delete, a swap
     write that must not resurrect a just-deleted file): has a filesystem side effect
     that must finish **even after the buffer leaves the workspace**. These are **not**
     dropped on close — the close path **awaits** them before removing the buffer
     entry (§4 close protocol).

   The job model marks each `JobKind`/result with which class it is; `apply_result`
   applies the drop rule only to buffer-local merges.

## 4. Lifecycle & UX

| Concern | Behavior |
|---|---|
| **CLI** | `wcartel a.md b.md c.md` opens all three as buffers, `active = 0`. `wcartel` alone → one scratch buffer. Each path resolved/opened with the existing 4a open logic + recovery-on-open per buffer. |
| **Open into new** | An `open <path>` command opens the file into a **new** buffer (does not replace the active one) and switches to it. Reopening an **already-open path** switches to its existing buffer — never two buffers over one file (which would fight over one swap). **Dedupe identity:** the **normalized absolute path** (lexical: absolutize against CWD + resolve `.`/`..`, no filesystem call), reconciled with `canonicalize()` when the file exists. This is well-defined for new/not-yet-created files (where `realpath` would fail); on first successful save/open the canonical form supersedes the lexical one. |
| **Switch** | `next_buffer` / `prev_buffer` cycle `active` (wrapping); on switch, **re-derive the newly-active buffer's view** (`derive::rebuild` + `ensure_visible`) so its layout/caret is current. Default keys deferred to Effort 5 keymap (proposed `Ctrl+PageDown`/`Ctrl+PageUp`). A palette-backed picker (Effort 5) lists buffers with name + dirty marker. |
| **Close** | `close_buffer` closes the active buffer. If it is **dirty**, raise a **close-confirm modal** — `[S]ave & close · [C]lose anyway · [Esc] cancel` — reusing 4b-2's `prompt` infra (a new `PromptAction::{SaveAndClose, CloseAnyway}` + a `Prompt::close_confirm()` constructor, carrying the target `BufferId`). On clean close, delete that buffer's swap (version-aware). **Close protocol vs. in-flight jobs (Codex review — the swap-writer-resurrection race):** if the buffer has an in-flight `SwapWrite`/`Save` (`swap_in_flight`), the close **defers removal**: it marks the buffer *closing*, **awaits the in-flight durability job** (bounded, like save&quit; terminal stays responsive), then deletes the swap and removes the buffer entry. A `SwapWrite` that completes for a *closing* buffer must **not** recreate the swap (its merge is a durability completion, not a buffer-local merge, and observes the closing state). This prevents a just-deleted swap from being resurrected by a writer that was already in flight. |
| **Close last** | Closing the **last** buffer leaves a **fresh empty scratch buffer** (the app never ends via close). Quit is the only path that exits. |
| **Status indicator** | The status line shows `[<active+1>/<count>] <name><dirty> [<mode>] <status>` — e.g. `[2/3] notes.md* [live] Saved`. With a single buffer the `[1/1]` prefix MAY be omitted to keep the current look. |
| **Status routing (multi-buffer)** | The status line is global, but background results target specific buffers. **Rule:** a transient status message from a result whose `buffer_id == active` shows as today; a result for an **inactive** buffer prefixes the buffer name (e.g. `other.md: swap write failed`) so it isn't mistaken for the active buffer. A **persistent per-buffer error** (e.g. a failed save) lives on the `Buffer` and is surfaced when that buffer becomes active, not silently overwritten by the active buffer's chatter. |
| **Quit (multi-dirty)** | `Ctrl+Q` with **any** dirty buffer → quit-confirm reporting the count: `N unsaved buffers — [S]ave all & quit · [Q]uit anyway · [C]ancel`. "Save all" dispatches a save per dirty buffer and bounded-joins on the **set** of awaited `(buffer_id, version)` results (generalizing 4b-2's `quit_after_save`). **Failure semantics (Codex review — no silent partial-save quit):** quit **only if every** awaited save succeeds at its awaited version. On **any** failure, timeout, or stale result, **stay open**, surface an error naming the failed buffer(s) (switching active to the first one), and **preserve all swaps**. `[Q]uit anyway` exits without saving (swaps survive for next-launch recovery). Clean buffers' swaps are deleted only on a successful exit. |
| **Per-buffer crash safety** | Each buffer keeps its **own** swap. Named buffers: path-hash-keyed (unchanged). **Scratch buffers re-keyed `scratch-<pid>-<bufferid>.swp`** — today's `scratch-<pid>.swp` collides once there is more than one scratch buffer in a process. Each buffer's cadence/`swap_in_flight` is independent (per-buffer fields), so an inactive dirty buffer keeps swapping on its own timer. |
| **Recovery-on-open (startup algorithm)** | Deterministic (Codex review): (1) open every CLI-named path + bare-scratch as buffers, `active = 0`, in CLI order; (2) enumerate **scratch orphans** from dead PIDs (the 4b-2 orphan finder, extended to parse the `-<bufferid>` suffix) — **each orphan becomes exactly one new scratch buffer** with a **fresh** `BufferId` and the orphan's `swap_path` attached as its `pending_recovery` (never collapse multiple orphans into one buffer); (3) for each named buffer, the hash-first predicate (unchanged) sets `pending_recovery` if its swap diverged; (4) present recovery prompts **one at a time in stable order** (CLI order, then orphans) — each prompt is buffer-scoped (`Prompt.target = Some(id)`), drives `active` to that buffer while live, and its action ([R]ecover/[D]iscard/[O]pen original) acts on that buffer's `pending_recovery`; cancel/Esc skips to the next. The user reaches the editor once the queue drains. |
| **Panic dump (known limitation)** | The panic dump stays **active-buffer-only** (per §8 non-goal): on panic it writes the **active** buffer's last-good snapshot. **Inactive dirty buffers are protected only by their independent swap cadence**, so a panic loses at most each inactive buffer's edits *since its last swap tick* — a window bounded by the swap interval (`T_idle` 2 s after the last edit, `T_max` 30 s under continuous editing). Dumping **all** dirty buffers on panic is a future refinement (§8). |

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
   `active = 0`, `next_buffer_id` counter + `alloc_id()`); `active()`/`active_mut()`/
   `by_id`/`by_id_mut` accessors; `new_from_text` builds exactly one buffer via
   `alloc_id()`. `Buffer` includes the relocated transients **and** `pending_recovery`
   (the recovery-on-open staging that moves off the global `Editor`); the recovery-open
   wiring in `app.rs` is updated to set/read the active buffer's `pending_recovery`
   (inert single-buffer behavior preserved). The `Document`/`View` structs are
   unchanged — only their ownership moves into `Buffer`.
2. **Migrate shell call sites** `editor.document.*` / `editor.view.*` /
   `editor.<transient>` / `editor.pending_swap_*` → through `active()`/`active_mut()`.
   Split across files to keep each change reviewable: (a) `editor.rs` + `derive.rs` +
   `nav.rs`; (b) `commands.rs` + `render.rs`; (c) `save.rs` + `swap.rs` + `app.rs`. ~2 tasks.
3. **Thread `buffer_id` through the job model:** add `buffer_id` to `Job`/`JobResult`
   and a result-class marker (buffer-local vs durability, §3.4); `apply_result` routes
   via `by_id_mut` (drop *buffer-local merges* if closed); `is_stale` keyed on
   `(buffer_id, version)`; save/swap merges target the resolved `&mut Buffer`.
   **Acceptance (Codex review):** the code is *mechanically* buffer-id-routed — no job
   path resolves `active()` at execution time — and a **debug assertion** confirms a
   result's `buffer_id` resolves to the intended buffer. Inert in *effect* with one
   buffer.
4. **Verify:** full workspace suite + 3× parallel + a manual binary smoke (open, edit,
   save, swap cadence, recovery, panic dump all behave exactly as before).

→ ~5 tasks; small, low-risk, gated entirely by the existing test suite.

### 6.2 Effort 6 — Multi-buffer workspace (post-1.0)

1. **N buffers + CLI multi-file open** (`wcartel a b c`, active=0) + the deterministic
   **recovery-on-open startup algorithm** (§4) using per-buffer `pending_recovery` and
   buffer-scoped prompts.
2. **Switch commands** `next_buffer`/`prev_buffer` + re-derive on switch + the
   `[i/n] name*` status indicator + the **status-routing rule** (inactive-buffer
   results prefixed by name; per-buffer persistent errors).
3. **Open-into-new** command + open-path dedupe by normalized-absolute identity
   (handles new/not-yet-saved paths; reopen → switch to existing).
4. **Close** command + close-with-dirty modal (`SaveAndClose`/`CloseAnyway`, target
   `BufferId`) + close-last → fresh scratch + the **close protocol** that awaits an
   in-flight durability job before removal (no swap resurrection).
5. **Per-buffer scratch-swap re-keying** (`scratch-<pid>-<bufferid>.swp`) +
   scratch-orphan enumeration (one fresh buffer per orphan) feeding the §4 startup
   algorithm.
6. **Quit with multiple dirty buffers** (save-all & quit; await the **set** of
   `(buffer_id, version)` results, bounded) with the **partial-failure semantics**
   (stay open + name the failed buffer + preserve swaps; never partial-quit).
7. **Job-routing-by-buffer-id made real:** integration test that a save/swap
   completing for a non-active or closed buffer merges into the right buffer / no-ops.
8. **Palette-backed buffer switcher** (depends on Effort 5 palette) — picker UI with
   name + dirty.
9. **Final review + crash-safety integration tests** (multi-buffer recovery, multi-dirty
   quit, swap isolation across buffers).

→ ~8–9 tasks. Only task 8 is gated on Effort 5.

## 7. Testing strategy

- **Prep refactor:** the existing suite is the gate — no behavior change permitted.
  Add focused unit tests for the new accessors (`active`/`by_id` invariants, `alloc_id`
  monotonicity / no reuse) and for job routing by id (with one buffer, a buffer-local
  result for that id merges; a result for a bogus/closed id no-ops; a durability result
  for a closed id still completes). The mechanical-routing debug assertion (§3.4) backs
  this.
- **Effort 6:** per-buffer isolation (edit/save/swap in buffer A leaves buffer B
  untouched); switch re-derive (B's caret/scroll restored on switch back); close-dirty
  modal matrix; close-last→scratch; open-path dedupe (incl. a **new/not-yet-saved**
  path, via the lexical-normalized identity); **multi-buffer recovery-on-open queue**
  (several pending recoveries presented in stable order, each acting on its own buffer
  via `pending_recovery` + `Prompt.target`); **scratch-orphan → one fresh buffer each**;
  **multi-dirty save-all-&-quit failure** (one save fails → stays open, names the
  buffer, preserves all swaps, no partial-quit); **status routing** (an inactive
  buffer's result prefixes its name, doesn't clobber the active status); **the close
  vs. in-flight-swap-writer race** (close a buffer with an in-flight `SwapWrite`: the
  swap is not resurrected after deletion — the close awaits the durability completion);
  **job routes to the correct buffer after a switch and no-ops (buffer-local) / still
  completes (durability) after a close** (the §3.3/§3.4 invariants). Determinism via
  `InlineExecutor` + injected `Clock`, as in 4b. Any swap-writing test uses unique temp
  paths (the 4b-2 parallel-isolation discipline).

## 8. Non-goals (explicit)

- **Split panes / windows** (multiple buffers visible at once) — designed-for, not
  implemented; a later sub-effort wraps buffers in a window-tree.
- **Session/workspace persistence** across runs (reopen the same set of buffers).
- **Tab-bar chrome** beyond the one-line status indicator.
- **Per-buffer independent render modes shown simultaneously** (only the active
  buffer renders; its own `view.mode` already persists per buffer).
- **Dumping all dirty buffers on panic** (the panic dump stays active-buffer-only;
  inactive dirty buffers are protected by their independent swap cadence, loss bounded
  by the swap interval — see §4 "Panic dump (known limitation)"). A future refinement
  makes `LAST_GOOD` a per-buffer map and dumps all dirty buffers.
- **Save-as / path change while a job is in flight** (file identity is immutable this
  effort; save-as is Effort 5, which must then add path identity to the job payload).

## 9. Spec → parent-section traceability

| This spec | Parent § | Item |
|---|---|---|
| §2, §8 | 5 Backlog | "Multiple buffers / windows" — post-v1, now split prep + feature |
| §3.1, §3.2 | 10.2 | flat `Editor` (Document/View split kept) → `Editor` over `Vec<Buffer>` |
| §3.3 | 10.3, 4b | async edges over snapshots; job staleness now `(buffer_id, version)` |
| §4 close/quit | 15.2, 4b-2 | modal only for destructive decisions; reuse swap/recovery + modal infra |
| §4 crash safety | 15.7, 4b-2 | per-buffer swap/recovery; scratch re-keying; orphan enumeration |
| §5 | 5, 18 | post-1.0 effort carrying a substrate requirement onto earlier efforts |
