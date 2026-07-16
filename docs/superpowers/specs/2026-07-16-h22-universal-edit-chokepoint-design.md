# H22 — Universal Edit Chokepoint (design spec)

**Status:** DRAFT — for Codex spec-gate review (cross-check against real source, not the diff).
**Effort:** H22 (unify-ad-hoc-surfaces arc; follows A17 messaging + H21 overlay dispatch).
**Author thread:** Fable warm authoring thread. **Date:** 2026-07-16.
**Top-level decision (human-locked, do NOT relitigate):** **B — a shared inner core that owns
the bookkeeping, with a thin validated shell on top.** `submit_transaction` becomes "validate,
then call the core"; internal callers call the core directly, pre-trusted.

---

## 1. Problem

Every internal buffer edit today reaches `Buffer::apply` (`editor.rs:267`) — which already owns
~80% of the per-edit bookkeeping — but each call site **hand-rolls the surrounding shell and
epilogue**, and they disagree:

- **Read-only guard is inconsistent.** `Editor::apply` (`editor.rs:1062`) is loud
  (`reject_read_only`, `editor.rs:1078`); `Buffer::apply` is silent (`editor.rs:268`); three search
  sites and `dispatch_transform` re-implement a loud entry guard by hand
  (`search_ui.rs:30/66/99`, `transform.rs:214`); `apply_filter_done` has **no read-only check at
  all** (`jobs_apply.rs:235–256`) yet still emits "filter applied" — a latent false-ack.
- **The reparse + viewport epilogue is hand-called and drifts.** `derive::rebuild` +
  `nav::ensure_visible` are re-typed at every site (`commands/edit.rs:23`, `jobs_apply.rs:251/365`,
  `search_ui.rs:61/116`, `transform.rs:298`, `scratch.rs:59`), sometimes **omitted**
  (`search_step_apply`, `scratch.rs:21`'s append), and `diag_apply_selected` rebuilds **twice**
  around an unfold (`search_ui.rs:209/211`) specifically to dodge a stale-tree read ordering.
- **Validation lives only in the untrusted shell** (`submit_transaction`, `transact.rs:22`); there
  is no debug backstop on the pre-trusted internal edits.

The cost is a maintenance/soundness hazard, not a live user bug: a new edit site must remember an
unwritten checklist, and the compiler enforces none of it. H22 routes **all** internal edits
through **one funnel** so versioning, the read-only guard, the reparse/viewport epilogue, and the
Effort-P plugin-edit seam live at a **single, compiler-guarded seam** — the anti-regrowth
"registration seam for edits" (CLAUDE.md → *Module structure*).

### Non-goals (kept explicit; see §9)
Undo/redo and whole-buffer replacement stay separate operations (different bookkeeping); the
history / `ChangeSet::apply` layer is untouched; per-site status messaging stays caller-owned;
async staleness-discard is not unified beyond an optional read-only helper; no `wc.*` API changes;
no swap/reconcile subsystem changes; the incremental-parser divergence tail is out of scope.

---

## 2. Current surface (grounding — every claim anchored)

### 2.1 What `Buffer::apply` already owns (the de-facto core), `editor.rs:267–305`
`pub fn apply(&mut self, txn: Transaction, edit: Edit, kind: EditKind, clock: &dyn Clock)`:
silent read-only early-return (`:268`); `history.commit_coalescing` undo push (`:272`); undo-eviction
capture into `undo_evicted_pending` (`:276–279`); `version += 1` (`:280`); `pre_edit_rope` /
`last_edit` (`:281–282`); `marks` + `jump_ring` `map_pos` remap (`:284–289`); `folds.remap`
(`:292`); `marked_block` / `pending_block_begin` remap + collapse-clear (`:295–303`);
`recovery::record_snapshot` of the new rope into the process-global `LAST_GOOD` panic latch
(`:304`, `recovery.rs:11–15`). It does **NOT** reparse (`derive::rebuild`) or move the viewport
(`nav::ensure_visible`).

**Correction to the grounding census:** the census said "recovery/swap snapshot is INSIDE apply."
Only the *recovery* half is. Swap **scheduling** is derived elsewhere from the version bump —
`swap::pending(dirty, version, swapped_version)` (`swap.rs:80`) — not written by `apply`. The
funnel neither contains nor changes swap logic.

### 2.2 The shell + delegator
- `submit_transaction` (`transact.rs:12`) — the ONLY validating path: `validate_against`
  (`transact.rs:22`), selection-snap on a clone (`transact.rs:29–34`), then `editor.apply`
  (`transact.rs:43`). It is already the **Effort-P plugin edit boundary**: `wc.replace` / `wc.insert`
  call it at `plugin/api.rs:349`, `:377`, `:404`.
- `Editor::apply` (`editor.rs:1062`) — thin loud delegator: `reject_read_only` + early return on a
  read-only active buffer (`:1063`), else `self.active_mut().apply(...)` (`:1064`).

### 2.3 The FULL edit surface (re-grounded — Codex round-1 correction)
Round 1 correctly found the census under-scoped this: mapping the **raw `Buffer::apply` bypasses**
missed the **`Editor::apply` delegator clients that hand-roll their own epilogue**. Under F2=A
(epilogue moves into the core that `Editor::apply` wraps), every such client would **double-run** the
epilogue unless migrated. Complete surface, re-derived by grepping every non-test `.apply(` with a
`Buffer`/`Editor` receiver (the test-only sites — `session_restore.rs:399/417`, `ventilate.rs:833/866`,
`workspace.rs:569/587`, `mouse.rs:2076`, `editor.rs:1540/1677/1743` — were confirmed inside
`#[cfg(test)] mod tests` and excluded):

**(A) Raw `Buffer::apply` bypass sites — 7** (call `Buffer::apply` directly, skip the delegator):

| # | site (`file:line`, fn) | target buffer | read-only today | epilogue today |
|---|---|---|---|---|
| 1 | `transform.rs:292` `merge_transform_into` | `by_id` (poss. non-active) | entry guard `transform.rs:214` | active-gated `:297–300` |
| 2 | `jobs_apply.rs:245` `apply_filter_done` | `by_id` (poss. non-active) | **none** | active-implicit `:251` |
| 3 | `jobs_apply.rs:361` `insert_paste_text` | `by_id` (poss. non-active) | loud `:342` | active-gated `:364–367` |
| 4 | `search_ui.rs:57` `search_replace_all` | `active_mut` | loud `:30` | `:61–62` |
| 5 | `search_ui.rs:79` `search_step_apply` | `active_mut` | loud `:66` | **omitted** (only `search_pin`) |
| 6 | `search_ui.rs:114` `search_step_rest` | `active_mut` | loud `:99` | `:116` |
| 7 | `scratch.rs:21` `append_to_scratch` | `by_id` (scratch, non-active) | none (silent) | **omitted** |

**(B) `Editor::apply` clients with a STANDARD hand-rolled epilogue** (all active-buffer; today do
`editor.apply(...)` then `settle_after_edit` or an equivalent rebuild+ensure_visible+`desired_col=None`):

| site (`file:line`, fn) | epilogue today |
|---|---|
| `commands/edit.rs` inserts/deletes — `:41,:52,:68,:81,:96,:113,:128,:148,:167,:186,:221,:246` | `settle_after_edit` (`commands/edit.rs:21–26`) |
| `commands/textops.rs` — `:47,:66,:122,:156,:208,:259,:291,:347,:405,:435` | `settle_after_edit` |
| `commands/prose_ops.rs:91` `move_sentence` | `settle_after_edit` `:92` |
| `commands/prose_ops.rs:197` `break_paragraph_here` | `settle_after_edit` `:198` |
| `commands/prose_ops.rs:250` `merge_paragraph_forward` | `settle_after_edit` `:251` |
| `commands/prose_ops.rs:293` `split_sentence_at_caret` | `settle_after_edit` `:294` |
| `blocks_marked.rs:88` (local `apply_edit` helper, used by `block_copy:30` + `block_delete:74` + `block_move:57` — **3 callers**) | `rebuild`+`ensure_visible`+`desired_col=None` `:89–91` |
| `scratch.rs:57` `move_block_to_scratch` (active source delete) | `rebuild`+`ensure_visible` `:59–60` |
| `search_ui.rs:208` `diag_apply_selected` (suggestion apply, via `Editor::apply`) | **double** `rebuild` around unfold `:209/:211` |

**(S) The untrusted SHELL — `submit_transaction` (`transact.rs:12`, applies at `:43`)** — the one
validating `Editor::apply` client. NOT an epilogue drop (`submit_transaction` never ran an epilogue —
it delegates to `Editor::apply`, which command primitives settle after). Under decision B it becomes
"validate → core" (§3.3): its `:43` `editor.apply(...)` becomes `edit_apply::apply_edit(...)`, so it
too stops touching `Buffer::apply` transitively. Listed here so INV-SEAM's source-scan test (§8.1)
accounts for every `Editor::apply`/`Buffer::apply` caller — including the plugin edit path that
reaches `Buffer::apply` only through this shell (`plugin/api.rs:349/377/404` → `submit_transaction`).

**(C) `Editor::apply` clients with a CUSTOM two-stage fold-correction epilogue — 2** (analyzed in §3.6):

| site (`file:line`, fn) | shape today |
|---|---|
| `blocks_marked.rs` `block_move` (fn ~`:30–68`) | `corrected_after_move:55` → `apply_edit` helper (apply + **rebuild #1**) `:57` → `replace_folded:59` → **rebuild #2** (bare) `:60` → `snap_caret_out_of_fold:64` |
| `commands/prose_ops.rs:101` `swap` | `corrected_after_move:150` → `editor.apply` (**Buffer::apply only, NO rebuild**) `:154` → `replace_folded:158` guarded by an explicit **rebuild #1** `:157` → `settle_after_edit` (**rebuild #2**) `:160` → `snap_caret_out_of_fold:164` |

These two are the **only** non-standard epilogues in the whole edit surface. `corrected_after_move` /
`replace_folded` / `snap_caret_out_of_fold` appear nowhere else in production
(`session_restore.rs:68` and `save.rs:282/330` use `replace_folded` for buffer-load restore, not edits
— out of scope). **Migration invariant (both commands):** `corrected_after_move` must stay computed
from the PRE-edit folds + `cs`, i.e. BEFORE `Transaction::new(cs)` moves `cs` and BEFORE the
`apply_edit`/core call (`blocks_marked.rs:54–56` before `:57`; `prose_ops.rs:149–151` before `:153/:154`).
The core migration must not reorder it after apply — the corrected set is defined against the pre-edit
fold byte-offsets, and moving it past apply would compute it against remapped offsets (§3.6).

### 2.4 The lazy-heal chain (why background staleness is safe — see F3, §6.2)
No background reconcile ever touches a non-active buffer: `dispatch_reconcile` (`reconcile.rs:40–45`)
and its arming (`app.rs:405–418`) are active-only. But `advance()` runs `derive::rebuild`
**pre-draw every loop iteration** (`app.rs:398`), and its parse phase is version-memoized (parse iff
`version != reconcile.blocks_version`, `derive.rs:96`; incremental iff exactly one behind with
`pre_edit_rope`/`last_edit`, `derive.rs:108–110`; else safe full parse). `workspace::switch_to`
(`workspace.rs:52–56`) rebuilds on every switch. Nothing in production reads a *non-active* buffer's
`blocks()` (outline/diag/ventilate/export/fold-view are all active-scoped). So an edit to a
non-active buffer leaves a lagging tree that is **healed before that buffer's first render**.

### 2.5 No hidden mutation route (verified)
`Buffer::apply` incoming production callers = the 7 sites + the `Editor::apply` delegator (`:1064`);
all others are `#[cfg(test)]`. The one non-`apply` direct write to `document.buffer`
(`plugin/api.rs:640`) is inside `#[cfg(test)] mod tests`. `ChangeSet::apply` (`change.rs`) is reached
only via the history layer + `submit_transaction`'s dry-run clone — no bypass touches it.

---

## 3. Design — the core / shell boundary

### 3.1 New module `wordcartel/src/edit_apply.rs` (F1 = B)
The Editor-level apply-core lives in a **new module**, not grown into the budgeted `editor.rs` hub.
This module IS the edit registration seam.

```rust
//! The one funnel every internal buffer edit passes through (H22). Owns: the loud read-only
//! guard, the single Buffer::apply call, and the reparse+viewport epilogue. `submit_transaction`
//! validates then calls here; internal callers call here pre-trusted. Buffer::apply is pub(crate)
//! and MUST NOT be called anywhere but this module (the compiler-guarded no-bypass seam).

/// Outcome of a funnelled edit — callers gate their status acks on this (F4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditOutcome {
    /// The edit committed.
    Applied,
    /// The target buffer is read-only; the canonical Sticky Warning was set here, nothing mutated.
    RejectedReadOnly,
    /// The target buffer id was not found (raced close/dispose); nothing mutated, no status.
    BufferGone,
}

/// THE internal-edit funnel. Applies `txn`/`edit` to buffer `buffer_id` (not necessarily active)
/// through the single mutation channel, then runs the epilogue. Pre-trusted: the changeset is
/// assumed valid-by-construction (built from a live doc_len); a debug_assert backstops that (F5).
pub fn apply_edit(
    editor: &mut Editor,
    buffer_id: BufferId,
    txn: Transaction,
    edit: Edit,
    kind: EditKind,
    clock: &dyn Clock,
) -> EditOutcome;
```

**Body (normative behavior; exact code is the plan's job):**
1. **Read-only guard (F4, uniformly loud).** If `by_id(buffer_id)` exists and is `read_only`, call
   `editor.reject_read_only()` (`editor.rs:1078`) and return `RejectedReadOnly`. If the id is absent,
   return `BufferGone` (no status). `Buffer::apply`'s own silent early-return (`editor.rs:268`) stays
   as defense-in-depth — it is now unreachable on the happy path but guards a future in-crate caller.
2. **Debug validation (F5).** Under `cfg(debug_assertions)`, before mutating, assert the changeset
   applies cleanly against the live target buffer. `ChangeSet::validate_against` takes a
   **`&TextBuffer`** (`change.rs:164`), and `by_id(buffer_id)` yields a `&Buffer` (`editor.rs:711`),
   so the argument is the buffer's inner text field:
   `debug_assert!(txn.changes.validate_against(&buf.document.buffer).is_ok(), "internal edit built an invalid changeset")`
   (C1 — the earlier `&buf` was a type error). Release builds stay trust-by-construction (zero cost)
   — the H7 blast-radius stance (debug-loud on the mutation path, no garbage in release).
3. **Mutate.** In a scoped `by_id_mut(buffer_id)` borrow, call `buffer.apply(txn, edit, kind, clock)`
   (the demoted `pub(crate)` method). The borrow ends before step 4 (the `transform.rs:290–300`
   borrow-split pattern, made canonical here).
4. **Epilogue (F2, core-owned).** Iff `buffer_id == editor.active().id`, run `resettle(editor)` —
   the shared epilogue: `crate::derive::rebuild(editor)`, `crate::nav::ensure_visible(editor)`, and
   `editor.active_mut().desired_col = None` (§3.2). Non-active edits skip the epilogue AND the
   `desired_col` reset (the lazy-heal invariant, §6.2 — the edited buffer's vertical anchor is stale
   only until it is next activated, where its first motion recomputes it; harmless). Return `Applied`.

### 3.2 `Editor::apply` becomes the active-buffer wrapper; the epilogue helper is SHARED (C2 fix)
`Editor::apply` (`editor.rs:1062`) keeps its call shape but delegates: resolve the active id, call
`edit_apply::apply_edit`, return the `EditOutcome`. (Signature widens from `()` to `EditOutcome`;
existing statement-position callers compile unchanged — see §11, judgment call J2.)

**The epilogue helper is retained and relocated, NOT deleted (round-1 C2 correction).** The three
epilogue steps — `derive::rebuild` + `nav::ensure_visible` + active-buffer `desired_col = None` —
are today packaged as `settle_after_edit` (`commands/edit.rs:21–26`, `pub(super)` at `commands`
scope). H22 makes this the **single core epilogue**, defined in `edit_apply.rs` as
`pub(crate) fn resettle(editor: &mut Editor)` so the core (a sibling module of `commands/`, which
`pub(super)` would not reach) can call it. Its three effects are unchanged. Consequences:
- The **core** runs `resettle` on the active-edit path (§3.1 step 4).
- The **standard `Editor::apply` clients** (surface B, §2.3) STOP calling `settle_after_edit` — the
  core now owns the epilogue; calling it again would double-run it. They end on
  `editor.apply(...)`'s outcome and return `CommandResult::Handled`.
- The **two custom-fold commands** (surface C) call `resettle` (or keep a bare `derive::rebuild`,
  see §3.6) as their post-`replace_folded` **second** rebuild — this is legitimate additional work
  after the core returns, not an accidental double-epilogue.
- `commands/edit.rs::settle_after_edit` is either removed or reduced to a `pub(super)` one-line
  delegate to `edit_apply::resettle` (plan's choice; behavior identical). This is a spec of the
  behavior, not the exact visibility mechanics.

### 3.3 `submit_transaction` = validate → core (decision B)
`submit_transaction` (`transact.rs:12`) keeps its untrusted-boundary contract — `validate_against`,
selection-snap, conservative whole-doc `Edit` — and its final step changes from `editor.apply(...)`
(`transact.rs:43`) to `edit_apply::apply_edit(editor, active_id, final_txn, edit, EditKind::Other,
clock)`, mapping the outcome to its `Result<(), EditError>` (a `RejectedReadOnly` on the active
buffer becomes `Ok(())` after the loud reject — the read-only view's edits are refused the same way
whether they arrive from a plugin or a keystroke). The three `wc.*` plugin call sites
(`plugin/api.rs:349/377/404`) are untouched.

### 3.4 The compiler-guarded no-bypass seam
`Buffer::apply` is demoted from `pub` to **`pub(crate)`** (`editor.rs:267`). This blocks every
out-of-crate caller at compile time — the Effort-P concern (a plugin, or any future external crate,
cannot reach the raw mutator; it must go through `submit_transaction` → core). In-crate, `edit_apply`
is a distinct module from `Buffer`'s definition (`editor.rs`), so `pub(crate)` cannot be narrowed to
"only `edit_apply`" by visibility alone without co-locating the core into `editor.rs` (rejected by
F1=B). The in-crate backstop is therefore a **source-scan invariant test** (§8.1) that fails if any
production file other than `edit_apply.rs` invokes `Buffer::apply`. Compiler for the external
boundary; scan test for in-crate regression.

### 3.5 F6 — close the undo/redo `LAST_GOOD` panic-latch gap
`Buffer::undo` (`editor.rs:306`) and `Buffer::redo` (`editor.rs:325`) mutate the rope but do **not**
call `recovery::record_snapshot`, so `LAST_GOOD` (`recovery.rs:8`) keeps the *pre-undo* rope until
the next `apply`; `dump_on_panic` (`recovery.rs:51–59`) after a run of undos would resurrect content
the user deliberately reverted. Fix: `undo`/`redo` call `recovery::record_snapshot(path, snapshot)`
on their success arm (after `version += 1`). This is the **one deliberate widening** of the
"undo/redo untouched" boundary — undo/redo otherwise stay OUT of the apply core (they replay an
existing changeset with clamp-not-remap folds, cleared marked_block, `last_edit = None`; §9).

### 3.6 The two custom-fold commands — F2=A HOLDS; no opt-out, no allow-list (round-1 verdict)
`block_move` (`blocks_marked.rs` ~`:30–68`) and `swap` (`prose_ops.rs:101–168`) both do a bespoke
two-stage fold correction across a move: compute a corrected fold set from the **pre-edit** folds +
the changeset (`corrected_after_move`, `blocks_marked.rs:55` / `prose_ops.rs:150`), apply the edit,
**rebuild #1** to settle the tree, `replace_folded` to override `Buffer::apply`'s plain remap with
the corrected set (`blocks_marked.rs:59` / `prose_ops.rs:158`), **rebuild #2** to relayout +
reconcile the corrected folds, then `snap_caret_out_of_fold`. The question F2 was about: does moving
the epilogue into the core force `swap` to opt out?

**Verdict: (a) — F2=A holds, and in its strongest form: neither command opts out or needs an
allow-list. Both migrate to the core; the core owns rebuild #1; each keeps its post-`replace_folded`
rebuild #2 as legitimate additional work.** Grounded reasoning:

- **`block_move` is living proof of the ordering.** It ALREADY runs "rebuild #1 (with the plain,
  UNcorrected apply-remap of folds, via its `apply_edit` helper's `rebuild` at `blocks_marked.rs:89`)
  → `replace_folded` → rebuild #2 → snap." That shipped sequence is exactly what the core produces:
  the core's `resettle` IS rebuild #1. So converging `swap` onto the same shape does not invent an
  untested ordering — it adopts one already in production. The intermediate reconcile against the
  UNcorrected folds is harmless: `replace_folded` discards it before rebuild #2 reconciles the
  corrected set.
- **`swap`'s `// Buffer::apply only — NO rebuild` (`prose_ops.rs:154`) was conservative, not
  necessary.** Its explicit rebuild #1 (`prose_ops.rs:157`, inside the `corrected` branch) is
  precisely the rebuild the core now runs unconditionally. Post-migration: `editor.apply(...)` (core
  runs rebuild #1) → drop the now-redundant `:157` rebuild → keep `replace_folded:158` →
  `settle`/`resettle` rebuild #2 `:160` → `snap:164`. The comment is removed.
- **Ordering equivalence, both paths.** Fold path: today = rebuild #1 (branch) + rebuild #2 = 2;
  post = core rebuild #1 + rebuild #2 = 2, `replace_folded` between — identical. No-fold path (swap):
  today = Buffer::apply(no rebuild) + `settle`(1 rebuild); post = core(1 rebuild) + `settle`(1
  rebuild, LayoutKey-gated no-op since nothing changed) — one extra gated no-op, same final state.
- **`replace_folded` bumps `folds.epoch`**, which is a `LayoutKey` input (`derive.rs` LayoutKey
  `fold_epoch`), so rebuild #2 is NOT gated away — it does real relayout/reconcile work. This is why
  it legitimately stays after the core call and is not a redundant double-epilogue.
- **Migration ORDERING invariant (must hold or the fold correction breaks).** `corrected_after_move`
  (`blocks_marked.rs:55` / `prose_ops.rs:150`) is computed from the PRE-edit folds + `cs` and MUST
  remain **before** `Transaction::new(cs)` and the `apply_edit`/core call — for `swap`, before
  `prose_ops.rs:153/:154`; for `block_move`, before `blocks_marked.rs:57`. The implementer must not
  reorder it after apply during migration: the corrected set is defined against pre-edit fold
  byte-offsets, so moving it past apply would compute it against post-edit remapped offsets and
  silently corrupt fold survival. `swap` running `resettle` twice on this path (core rebuild #1 +
  post-`replace_folded` rebuild #2) is the INTENDED shape, not a bug.

Consequence for INV-SEAM: because both commands route through the core (not raw `Buffer::apply`),
the no-bypass seam stays **pure — `edit_apply` is the sole `Buffer::apply` caller, with no allow-list
entries.** (I evaluated the coordinator's allow-list form of (a) — swap stays a raw allow-listed
`Buffer::apply` caller — and rejected it: full migration onto `block_move`'s proven shape is safe, so
a bypass is unwarranted and keeps the scan test's allowlist a single entry.) **I did NOT reopen F2**;
this is (a), folded. The only residual behavior note (confirm at review): if the plan repoints
`block_move`'s bare rebuild #2 (`blocks_marked.rs:60`) at the shared `resettle`, it gains a harmless
`ensure_visible` + `desired_col=None`; the conservative alternative is to leave it a bare
`derive::rebuild`. Either is behavior-preserving; the plan picks one.

---

## 4. Named invariants the funnel establishes

- **INV-SEAM (no-bypass).** `Buffer::apply` is `pub(crate)` and the sole production caller is
  `edit_apply::apply_edit`. Enforced by the compiler (external) + the §8.1 scan test (in-crate).
- **INV-GUARD (uniform loud read-only, F4).** Every internal edit that targets a read-only buffer
  returns `RejectedReadOnly` with the canonical Sticky Warning set once; no caller emits a success
  ack on that outcome. `Buffer::apply`'s silent return remains as defense-in-depth.
- **INV-EPILOGUE (F2).** After an edit to the **active** buffer, the tree is reparsed and the caret
  made visible before control returns to the caller; callers do not hand-roll the epilogue. The
  `desired_col = None` reset is owned by `resettle` — **no edit path writes `desired_col` directly
  outside `resettle`.** This is an ownership rule, not a "reset exactly once" rule: `resettle` is
  idempotent for `desired_col`, and the two fold-commands (§3.6) intentionally run it twice on a
  folded-region move (core rebuild #1, then the post-`replace_folded` rebuild #2), both setting
  `desired_col = None` — benign, the final state is identical. (Non-edit paths — cursor movement,
  undo/redo caret-snap, selection — still set `desired_col` directly; they are outside this
  chokepoint's scope, which governs edits only.)
- **INV-LAZY-HEAL (F3).** A non-active edited buffer's tree MAY lag (`reconcile.blocks_version <
  document.version`); every activation path — `advance()` pre-draw `derive::rebuild` (`app.rs:398`)
  and `workspace::switch_to` (`workspace.rs:54`) — heals it before its first render. This is the
  deliberate proportional-to-work choice (CLAUDE.md → *Resource behavior — proportional to work,
  free at rest*): eager reparse of a possibly-never-viewed buffer would spend O(document) off the
  hot path for no observable benefit.

---

## 5. Complete migration map (intent; the plan enumerates exact edits)

The full edit surface from §2.3. Every site routes through the core; each drops its hand-rolled
epilogue (now core-owned) and gates its status ack on the returned `EditOutcome`. No production
`Buffer::apply` caller survives except `edit_apply` itself.

### Surface A — raw `Buffer::apply` bypasses → `edit_apply::apply_edit`
- **`merge_transform_into` (`transform.rs:262`):** replace `by_id_mut`→`apply` + active-gated
  `rebuild`/`ensure_visible` (`:291–300`) with `apply_edit(editor, buffer_id, …)`; keep the no-op
  check (`:282`) and `finish_topic` acks, gated on `Applied`.
- **`apply_filter_done` (`jobs_apply.rs:211`):** replace `by_id_mut`→`apply`→`bool` + active-gated
  epilogue (`:236–255`) with `apply_edit`; emit "filter applied" **only on `Applied`** (kills the
  false-ack class — a read-only target now yields `RejectedReadOnly`, not a lie).
- **`insert_paste_text` (`jobs_apply.rs:336`):** keep the paste-size + entry read-only guard
  (`:342–351`, work-avoidance + register-gating); replace the `by_id_mut` mutation + epilogue
  (`:353–368`) with `apply_edit`; the `bool` return becomes `matches!(outcome, Applied)`.
- **`search_replace_all` / `search_step_apply` / `search_step_rest` (`search_ui.rs:29/65/98`):**
  keep the entry read-only guards (they prevent a false "Replaced N" / wasted plan build); replace
  `active_mut().apply` + trailing `rebuild`/`ensure_visible` (or the omission in `search_step_apply`)
  with `apply_edit`. `search_pin` (`search_ui.rs:125`) stays — the `apply_edit` epilogue makes the
  pin's own `rebuild` a LayoutKey-gated no-op, not a correctness dependency.
- **`append_to_scratch` (`scratch.rs:11`):** replace `by_id_mut(sid).apply` (`:21`) with
  `apply_edit(editor, sid, …)`. Scratch is non-active → INV-LAZY-HEAL (no epilogue; heals on next
  `toggle_scratch`/switch).

### Surface B — `Editor::apply` clients with a STANDARD epilogue → drop the epilogue (core owns it)
- **Command primitives** `commands/edit.rs` (`:41…:246`) + `commands/textops.rs` (`:47…:435`): keep
  `editor.apply(...)` (now full-epilogue); STOP calling `settle_after_edit`; return
  `CommandResult::Handled`. No behavior change (active-buffer; epilogue identical).
- **`move_sentence` / `break_paragraph_here` / `merge_paragraph_forward` / `split_sentence_at_caret`
  (`prose_ops.rs:91/197/250/293`):** drop each trailing `settle_after_edit` call (`:92/:198/:251/:294`);
  keep the `set_status` ack.
- **`blocks_marked.rs::apply_edit` helper (`:80–91`):** delete its manual
  `rebuild`+`ensure_visible`+`desired_col=None` (`:89–91`) — the core now owns them; the helper
  becomes a thin `editor.apply(...)` wrapper (or is inlined). Its **three** callers `block_copy:30`,
  `block_delete:74`, and `block_move:57` are unaffected in shape (the first two are standard; the
  third is Surface C).
- **`move_block_to_scratch` (`scratch.rs:42`):** the active source delete (`:57`) drops its explicit
  `rebuild`/`ensure_visible` (`:59–60`). (The non-active `append_to_scratch` half is Surface A.)
- **`diag_apply_selected` (`search_ui.rs:135`):** the suggestion apply (`:208`, currently via
  `Editor::apply`) uses `apply_edit`; the **double `rebuild`** around the unfold (`:209/:211`)
  collapses to a single `unfold_ancestors_of` + `ensure_visible` (the core already reparsed on the
  correct post-edit tree — the ordering hack is obsolete).

### Surface C — the two custom-fold commands (§3.6): core owns rebuild #1, keep rebuild #2
- **`block_move` (`blocks_marked.rs` ~`:30–68`):** `apply_edit` helper → core (rebuild #1); KEEP
  `replace_folded:59` + rebuild #2 (`:60`, bare `derive::rebuild` or repointed at `resettle` — §3.6)
  + `snap_caret_out_of_fold:64`.
- **`swap` (`prose_ops.rs:101`):** `editor.apply:154` → core (rebuild #1, subsumes the explicit
  `:157` rebuild, which is DELETED); KEEP `replace_folded:158` + rebuild #2 via `settle`/`resettle`
  (`:160`) + `snap_caret_out_of_fold:164`; remove the obsolete `// Buffer::apply only — NO rebuild`
  comment.
- **Ordering invariant (both, §3.6):** keep `corrected_after_move` (`blocks_marked.rs:55` /
  `prose_ops.rs:150`) computed BEFORE `Transaction::new(cs)`/the core call — do NOT reorder it after
  apply during migration, or the corrected fold set is computed against post-edit offsets.

### Surface S — the untrusted shell → validate → core (decision B, §3.3)
- **`submit_transaction` (`transact.rs:12`):** its terminal `editor.apply(...)` (`:43`) becomes
  `edit_apply::apply_edit(editor, active_id, …)`, mapping `EditOutcome` to `Result<(), EditError>`.
  Validation, selection-snap, and the conservative whole-doc `Edit` are unchanged. The three `wc.*`
  plugin sites (`plugin/api.rs:349/377/404`) are untouched — they reach the core only through this
  shell, so INV-SEAM covers the plugin edit path transitively.

---

## 6. Detailed decisions

### 6.1 Read-only guard placement (F4)
The loud guard is at the funnel (INV-GUARD). Existing **entry** guards that avoid *scheduling work*
stay: `dispatch_transform`'s guard (`transform.rs:214`) prevents spawning a worker thread;
`search_*`'s guards prevent building a replacement plan and a false ack. These are defense +
efficiency, now backstopped by the funnel rather than load-bearing. `apply_filter_done` needs no new
entry guard — its ack is gated on `Applied`.

### 6.2 Non-active reparse policy (F3) — LAZY, tested
Reaffirmed by the human as a responsiveness/resource decision. The funnel does **not** eager-reparse
non-active buffers; INV-LAZY-HEAL names and §8.2 pins the heal. This deliberately contradicts the
grounding census's "reparse the edited buffer ALWAYS" recommendation.

### 6.3 What stays caller-owned
Per-site status acks (`finish_topic(Transform/Filter, …)`, "Replaced N", "block moved to scratch")
are A17 feature-level routing and stay at the call sites, now gated on `EditOutcome`. The funnel owns
exactly one status effect: the canonical `reject_read_only` Warning on `RejectedReadOnly`.

### 6.4 Async staleness — unchanged (optional helper only)
The captured-version discard stays at each async merge entry: `apply_filter_done`
(`jobs_apply.rs:225`), `apply_transform_done` (`jobs_apply.rs:270`), the reconcile merge closure
(`reconcile.rs:64`). Semantics differ per caller (filter/transform discard against a captured
version; paste has none; reconcile merges a tree) and cannot be absorbed into the core without
inventing a version parameter sync callers lack. An **optional** `is_stale_for(&Editor, buffer_id,
version) -> bool` helper MAY factor the two identical `by_id(..).map(..version) != Some(version)`
checks (`jobs_apply.rs:225`, `:270`) — a readability nicety, not a behavior change, and explicitly
the ONLY unification in scope.

---

## 7. Command-surface-contract conformance — **N/A, with caveat**

H22 adds, removes, and renames **no** commands, user-settable options, palette rows, menu entries, or
keybinding hints. The migration sites are message handlers and command *bodies*; the registry
(`registry.rs`), palette, menu, and hint resolution are untouched. The contract's invariant tests
(palette-completeness, every-option-has-a-command, hint re-resolution) are unaffected.

**Caveat (stated precisely):** the F4 change makes previously-silent read-only edit paths
(`apply_filter_done`; `append_to_scratch`) emit the canonical read-only Warning. This changes status
**feedback** on paths that are **unreachable today** on a read-only buffer (`read_only` is set only
at the `view_messages` history buffer's construction, `editor.rs:193`; filter/scratch dispatch cannot
target it). It touches no command/option/palette/menu/hint surface and no contract law. Recorded here
so the gate sees it was considered, not omitted.

---

## 8. Testing strategy

All new tests are `#[cfg(test)]` unit tests unless noted; AAA; mock the clock via the existing
`TestClock`. Failing-test-first per task (TDD).

### 8.1 INV-SEAM — no-bypass (source-scan test, new integration test file)
Modeled on `tests/module_budgets.rs`'s file-read approach (`module_budgets.rs:15–46`). Scan every
`wordcartel/src/**/*.rs` **production** region (lines before a co-located `mod tests`, reusing that
file's `production_lines` idea) for a direct `Buffer::apply` invocation; assert the only production
file that contains one is `edit_apply.rs`. Honest limitation documented in the test: it is a
heuristic receiver-pattern scan (the `.apply(` token is shared by four types — census §"Count
reconciliation"), so it allowlists by *file*, not by parsing types; combined with the `pub(crate)`
compiler guard for the external boundary, it is sufficient to catch an in-crate regression. A second
assertion: `Buffer::apply`'s definition carries `pub(crate)` (grep the signature line) so a future
widen-to-`pub` trips the test.

### 8.2 INV-LAZY-HEAL — F3 (`edit_apply.rs` tests)
`apply_edit` into a **non-active** buffer leaves a lagging tree, and an activation path heals it:
1. Build a 2-buffer editor; edit the non-active buffer's content via `apply_edit(editor,
   background_id, …)`. Assert `document.version` bumped **and** `reconcile.blocks_version <
   document.version` (tree lagging) **and** the background `blocks()` still differs from
   `full_parse` of its new text.
2. `workspace::switch_to(editor, background_idx)`; assert `blocks() == full_parse(text)` and
   `reconcile.blocks_version == document.version` (healed before first render).
A real-instance variant drives `scratch::copy_block_to_scratch` (non-active scratch) and then
`workspace::goto_scratch`, asserting the same heal. Also assert the **active** edit path leaves NO
lag (epilogue ran): after `editor.apply(...)`, `reconcile.blocks_version == document.version`.

### 8.3 INV-GUARD / false-ack — F4 (`jobs_apply.rs` + `edit_apply.rs` tests)
- `apply_edit` into a read-only buffer returns `RejectedReadOnly`, mutates nothing, and sets
  `status_text() == "buffer is read-only"`.
- `apply_filter_done` targeting a read-only buffer (drive it directly with a `RunResult::Stdout` and
  `read_only = true`) emits **no** "filter applied" and the buffer is byte-identical — the census's
  false-ack class, now structurally impossible.
- Regression: the existing loud-reject tests
  (`transform::dispatch_transform_on_read_only_is_rejected`, `jobs_apply::paste_into_a_read_only_
  buffer_is_a_loud_reject`) stay green.

### 8.4 F6 — snapshot on undo/redo (`editor.rs` tests, reading `recovery::LAST_GOOD`)
Apply "X" to "abc\n" (LAST_GOOD → "Xabc\n"); `undo()`; assert `recovery::LAST_GOOD` now holds the
**post-undo** rope ("abc\n") — matching `active().document.buffer` — not the stale pre-undo content.
Repeat for `redo()`. (Locks `dump_on_panic` cannot resurrect undone content.)

### 8.5 Epilogue-equivalence regression (surface B)
The existing per-site epilogue tests must stay green through migration:
`derive::keystroke_runs_layout_once` (`derive.rs:803`), the search/transform/filter merge tests, the
`prose_ops` `move_sentence`/`break_paragraph`/`merge_paragraph`/`split_sentence` batteries
(`prose_ops.rs` `mod tests`), the `blocks_marked` `block_delete`/`block_jump` tests, and `scratch`'s
copy/move battery (`scratch.rs:75–148`). The e2e journeys (`e2e.rs`) and PTY smoke suite
(`scripts/smoke/run.sh`, mandatory-run/advisory-pass) exercise the real `reduce → advance → render`
loop end-to-end.

### 8.6 Custom-fold-correction regression (surface C, §3.6)
The two custom-fold commands must survive migration byte-identically — their fold survival has
prior gate-caught bugs (memory: S4 "fold survival via `replace_folded`+`map_pos`+snap-out"). Keep
green the existing `block_move` and `swap` fold-survival tests (`blocks_marked.rs` `mod tests`
around `:265`, `prose_ops.rs` `mod tests` around `:519`), which assert a folded region's fold
survives the move/swap and the caret snaps out of any hidden line. Add one focused assertion per
command: after the migrated path, the corrected fold set equals the pre-migration corrected set for
a folded-region move/swap (proves the core's rebuild #1 did not perturb the `replace_folded`
override) — grounded on `corrected_after_move`'s existing `fold.rs:536–564` fixtures.

---

## 9. Out of scope (explicit)

- **Undo/redo** (`Buffer::undo`/`redo`, `editor.rs:306`/`:325`) stay OUT of the apply core — genuinely
  different bookkeeping (clamp-not-remap folds, cleared `marked_block`, `last_edit = None`, no
  marks/jump_ring remap). The **only** change is the F6 `record_snapshot` line (§3.5).
- **`replace_buffer`** (`editor.rs:1091`) — whole-buffer swap; shares only the read-only guard with
  content-apply (incoming `Buffer` carries its own history + fresh parse). Untouched.
- **History / `ChangeSet::apply` layer** (`history.rs`, `change.rs`) — untouched.
- **Per-site status messaging** — stays caller-owned (§6.3).
- **Async staleness-check unification** — beyond the optional `is_stale_for` helper (§6.4).
- **Any `wc.*` plugin API change** — the plugin edit path is unchanged (`plugin/api.rs` untouched;
  it already routes through `submit_transaction`).
- **Swap / reconcile subsystems** — unchanged.
- **The incremental-parser `incremental ≡ full` divergence tail** — separate backlog item.

---

## 10. Module-size / anti-regrowth conformance (GATE)

- `edit_apply.rs` is a **thin seam**, not a dispatch hub: `apply_edit` is a short guard→mutate→epilogue
  function plus the shared `resettle` epilogue helper (both well under the `too_many_lines` = 100
  threshold, `clippy.toml`); the module holds those two fns plus `EditOutcome` and its tests. No
  `match`/loop dispatch attractor.
- The migration **shrinks** the hubs it touches — it removes ~30 per-site epilogue calls (`surface B`),
  the two `search_ui.rs` epilogue tails, `diag_apply_selected`'s extra rebuild, and relocates
  `settle_after_edit`'s body into `edit_apply::resettle` (a move, not net growth). Net negative
  production lines in `commands/*`, `search_ui.rs`, `jobs_apply.rs`, `transform.rs`, `scratch.rs`,
  `blocks_marked.rs`. `app.rs` / `render.rs` / `timers.rs` / `plugin/host.rs` budgets
  (`tests/module_budgets.rs`) are untouched.
- **Judgment call J1 (confirm):** do NOT add a `module_budgets` row for `edit_apply.rs`. That test
  bounds *god-hubs* (dispatch attractors); a ~60-line leaf seam does not belong there, and
  `too_many_lines` already guards its single function. (If you prefer a row for symmetry, say so and
  the plan adds one with headroom.)
- Effort-P conformance: the funnel is the edit registration seam — plugin edits enter through
  `submit_transaction` → core and add zero bulk to it.

---

## 11. Risks & judgment calls for human confirmation

- **J1 — no `module_budgets` row for `edit_apply.rs`** (§10). Recommended: none.
- **J2 — `Editor::apply` return type widens `()` → `EditOutcome`.** Statement-position callers
  (all command primitives) compile unchanged; only sites that want the signal read it. Alternative:
  keep `Editor::apply` returning `()` and have the async/search sites call `edit_apply::apply_edit`
  directly for the outcome. Recommended: widen (one funnel, one return shape).
- **J3 — in-crate no-bypass is a source-scan test, not pure compiler** (§3.4/§8.1), because F1=B puts
  the core in a separate module from `Buffer`'s definition, so `pub(crate)` can't be narrowed to one
  module. Recommended: accept (compiler covers the external/plugin boundary that actually matters;
  scan test covers in-crate regressions). Alternative if you want compiler-only: co-locate the core
  into `editor.rs` (rejects F1=B).
- **J4 — keep the entry-level read-only guards** in `dispatch_transform` / `search_*` as
  work-avoidance even though the funnel backstops them (§6.1). Recommended: keep.
- **J5 — `block_move`'s rebuild #2 (`blocks_marked.rs:60`) repointed at `resettle` vs left bare**
  (§3.6). Repointing adds a harmless `ensure_visible`+`desired_col=None`; both behavior-preserving.
  Recommended: repoint for one epilogue definition. Plan's discretion.
- **F2 swap verdict = (a), strongest form** (§3.6): F2=A holds unchanged; `swap` migrates to the
  core (no opt-out, no allow-list), because `block_move` already ships the exact
  rebuild#1→`replace_folded`→rebuild#2 ordering. I did NOT reopen F2. Flagged here so the human sees
  the allow-list alternative was considered and rejected as unnecessary.
- **F3 contradicts the grounding census** (lazy vs eager reparse). Human reaffirmed lazy;
  recorded as INV-LAZY-HEAL and tested. Flagged here per findings discipline.

---

## 12. Self-review pass

- **Placeholders:** none — every anchor is a real `symbol` + `file:line` verified against the tree on
  2026-07-16 (`Buffer::apply` `editor.rs:267`; `Editor::apply` `:1062`; `reject_read_only` `:1078`;
  `submit_transaction` `transact.rs:12` + plugin callers `plugin/api.rs:349/377/404`; the full edit
  surface in §2.3; `validate_against(&self, &TextBuffer)` `change.rs:164`; `by_id` `editor.rs:711`;
  `corrected_after_move`/`replace_folded`/`snap_caret_out_of_fold` at `blocks_marked.rs:55/59/64` +
  `prose_ops.rs:150/158/164`; `recovery::record_snapshot` `recovery.rs:11`; `swap::pending`
  `swap.rs:80`; heal chain `app.rs:398` + `workspace.rs:54` + `derive.rs:96`). The test-only `.apply`
  sites were confirmed under `#[cfg(test)]` and excluded (§2.3).
- **Round-1 findings folded:** C1 (validate_against `&buf.document.buffer`, §3.1); C2
  (`settle_after_edit` retained as the shared `resettle` epilogue, NOT deleted, §3.2); I1/I2/I3
  (`blocks_marked.rs:88`, `move_block_to_scratch:57`, prose_ops `:91/:197/:250/:293` all in the §2.3
  surface + §5 map); swap = (a) with full analysis (§3.6).
- **Round-2 findings folded:** I-A (`submit_transaction` `transact.rs:43` now listed as Surface S in
  §2.3 + §5, so INV-SEAM's scan accounts for every `Editor::apply`/`Buffer::apply` caller incl. the
  plugin path via the shell); M-1 (`blocks_marked::apply_edit` = **3** callers — `block_copy:30` +
  `block_delete:74` + `block_move:57` — §2.3/§5); M-2 (`desired_col` reworded to an ownership rule —
  no direct writes outside `resettle`; the two fold-resettles are the intended idempotent shape;
  non-edit paths out of scope — INV-EPILOGUE §4); migration ORDERING invariant pinned
  (`corrected_after_move` stays pre-apply — §3.6, §5).
- **Contradictions:** the census's "swap inside apply" and "reparse ALWAYS" claims are explicitly
  corrected (§2.1, §6.2). The earlier draft's "`settle_after_edit` is deleted" is corrected
  everywhere (§3.2, §5, §10, below). Decision B and F1–F6 are stated once and applied consistently.
- **Scope:** §9 fences undo/redo (bar the F6 line), `replace_buffer`, history layer, messaging, async
  unification, `wc.*`, swap/reconcile subsystems, and the parser tail — matching the coordinator's
  OUT list. The `prose_ops::swap` command is IN scope (a migration site); the `swap` *subsystem*
  (`swap.rs`, SSD-wear) is OUT — distinct meanings of the word, disambiguated at each use.
- **Ambiguity:** the core's active-gate (`buffer_id == active().id`), the debug-only validation, and
  the caller ack-gating are stated normatively; exact per-site edits, the `resettle` visibility
  mechanics, and J5 are deferred to the plan (this is a spec, not the plan).
```
