# C4 — close-buffer Save/Discard/Cancel prompt

**Status:** draft — pending Codex + Fable spec review
**Effort:** C4 (backlog Theme C; `needs-design` → designed, Small-Medium; the Effort 6
spec-conformance gap deferred 2026-06-28, pulled into the backlog 2026-07-04)
**Date:** 2026-07-04 · **Facts as of:** `43fcedf` (post-B1+B2 merge)

## Why

`workspace::close_buffer` (workspace.rs:99-123) REFUSES to close a dirty buffer
(status `"unsaved changes — save or discard first"`) instead of the interactive
Save/Discard/Cancel prompt the Effort 6 spec called for — safe, never loses work, but a
dead end: the user must save or manually revert before close works. C4 replaces the
refusal with the prompt, reusing the quit machinery's per-buffer save-on-close path
(`dispatch_save_then` + `pending_after_save`, the `ContinueQuitDrain` pattern) so the
async-save staleness discipline is inherited, not reinvented.

## Decisions (user-approved 2026-07-04)

1. **Discard leaves the swap file** (fork 1 = A): quit's discard paths leave swaps as the
   recovery net (the only existing discard precedent — `swap::delete` fires only on
   successful save, save.rs:100-113, and explicit recovery-discard, prompts.rs:170); close
   matches. One discard convention: a Discard keypress is never irreversible — reopening
   the file offers the discarded changes back.
2. **`ctrl-w` binds to `close_buffer`** (fork 2 = A): the near-universal close key, unbound
   today (keymap.rs grep-verified). Menu/palette hints re-derive automatically per the
   three-surface contract (registry = truth); the registry entry itself (registry.rs:282 —
   id `close_buffer`, label `Close Buffer`, `MenuCategory::File`) is unchanged.

## Design

### D1. The prompt (prompt.rs)

New constructor in the house idiom (beside `quit_review_buffer`, prompt.rs):

- `Prompt::close_confirm(name: &str)` — message
  `format!("Close {name}: unsaved changes — [S]ave & close  [D]iscard  [C]ancel")`
  (exact copy tuned at implementation to match the sibling constructors' style), choices:
  - `('s', "Save & close", PromptAction::CloseSave)`
  - `('d', "Discard", PromptAction::CloseDiscard)`
  - `('c', "Cancel", PromptAction::Cancel)`

Two new `PromptAction` variants: `CloseSave`, `CloseDiscard` (prompt.rs:6-27 enum).
`Cancel` is reused — its existing resolve arm clears `pending_export`/`pending_save_*`/
`quit_drain` (prompts.rs), all `None` in the close flow, so it is a harmless superset.
Esc routes through the existing prompt-Esc arm (app.rs:705-708) — same harmless-superset
argument. Key routing (`action_for`, case-insensitive) and status-row rendering
(render.rs:653-655) need no changes.

Because prompts are modal (the `editor.prompt.is_some()` guard intercepts ALL key input,
app.rs:692-737, and `open_prompt` clears every other overlay, editor.rs:592-604), the
active buffer cannot change between raise and resolve — `active()` at resolve time is the
buffer that raised the prompt. The ASYNC window opens only after `CloseSave` dispatches
the save; that window is handled by D3's explicit id capture.

### D2. The trigger + the shared close mechanics (workspace.rs)

`close_buffer` (:99-123) changes ONLY its dirty guard (:102): instead of setting the
refusal status, it calls
`editor.open_prompt(crate::prompt::Prompt::close_confirm(&name))` where `name` is the
buffer's display name (the same name source `quit_review_buffer`'s callers use). The
scratch guard (:101) stays FIRST and unchanged — the scratch buffer never closes, dirty
or not. The clean path is extracted, not changed:

- New `pub(crate) fn close_buffer_now(editor: &mut Editor, id: BufferId)` — the existing
  clean-path mechanics (:103-122) generalized to work BY ID rather than on the active
  buffer: locate the buffer's index by id (if the id no longer exists, set a status and
  return — the vanished-buffer case); the last-ordinary-buffer check
  (`ordinary <= 1` → replace in place with a fresh untitled, prune MRU, rebuild,
  ensure_visible) and the normal path (`mru.retain`, `buffers.remove`,
  `new_idx = a.min(len-1)`, `switch_to`) move here verbatim-modulo-the-id-lookup.
  The last-ordinary count is computed AT CALL TIME — never cached from prompt-raise time
  (the user may have closed other buffers while a save was in flight).
- `close_buffer`'s clean path becomes `close_buffer_now(editor, active_id)`.

Three callers of `close_buffer_now`: the clean path above, D4's `CloseDiscard` arm, and
D3's post-save arm.

### D3. The save path (editor.rs, save.rs, prompts.rs, jobs_apply.rs)

- **`PostSaveAction` gains `CloseBuffer { id: BufferId }`** (editor.rs:17, today
  `Quit | ContinueQuitDrain`). The id is captured EXPLICITLY: the save is async and the
  user can switch buffers mid-flight; `apply_result` must act on the captured id via
  `by_id`-style lookups, never `active()`. (`PendingAfterSave` already carries
  `buffer_id` + `version`, editor.rs:36-41 — the variant's id is for the CLOSE action;
  the staleness match keys on the existing fields exactly as the Quit arm does.)
- **`resolve_prompt` gains two arms** (prompts.rs, beside `ReviewSave`/`ReviewDiscard`):
  - `CloseSave` → clear the prompt; capture `id = active buffer id`;
    `crate::save::dispatch_save_then(ctx, PostSaveAction::CloseBuffer { id })` — exactly
    `ReviewSave`'s shape. The unnamed-buffer case rides the EXISTING carry:
    `dispatch_save_then` sets `pending_save_as = Some(action)` when Save-As opens
    (save.rs:169-184), and `perform_save_as` arms `pending_after_save` from it
    (prompts.rs:84-86) — no new code, the variant flows through. **Save-As divergence
    note:** the Save-As minibuffer is NOT modal — the user can switch buffers before
    submitting, so `perform_save_as` may arm `PendingAfterSave` for a different buffer
    than the action's captured `id`. No data loss is possible: the apply arm checks
    `is_dirty(id)` on the ACTION's id, and a still-dirty original takes the
    close-cancelled branch. (The wrong-buffer-saved exposure is the quit flow's
    pre-existing Save-As semantics, unchanged by C4.)
  - `CloseDiscard` → clear the prompt; `crate::workspace::close_buffer_now(editor, id)`
    (active id — modal window, per D1). No executor needed. The swap file is NOT deleted
    (decision 1).
- **`apply_result` gains a third `pending_after_save` arm** (jobs_apply.rs:26-68, beside
  the `Quit` and `ContinueQuitDrain` arms, same `saved_this` discipline):
  - `saved_this && !is_dirty(id)` → clear `pending_after_save`;
    `crate::workspace::close_buffer_now(editor, id)`.
  - `saved_this` but dirty again (edited during flight) → clear `pending_after_save`;
    status `"edited during save — close cancelled"` (the Quit arm's convention verbatim,
    with "close" for "quit"); do NOT close.
  - `!saved_this` (save failed) → clear `pending_after_save`; do NOT close; the save
    merge's own error status stands (the Quit arm's failure convention).
- **The timeout tick arm** (app.rs:1423-1444, `SAVE_QUIT_TIMEOUT_MS`) gains the
  `CloseBuffer` disposition: clear `pending_after_save`, status
  `"save timed out — close cancelled"` (mirrors `ContinueQuitDrain`'s wording with
  "close"; no modal re-raise — unlike the `Quit` variant's re-prompt, a close is not a
  session-ending action the user is waiting on).
- **`apply_panic`** (jobs_apply.rs:92-125) already clears `pending_after_save`
  unconditionally — the new variant is covered with zero changes; verify, don't modify.

### D4. The keybinding (keymap.rs)

`ctrl-w` → `close_buffer` in the default binds. If the keymap has per-preset tables
(cua/wordstar), bind in the DEFAULT/cua table only; the wordstar preset question is out
of scope (`ctrl-w` may carry legacy meaning there — D1/A5 territory). Hints re-derive.

## What does NOT change

- The registry entry (id/label/menu) — the three-surface contract needs nothing.
- The quit machinery: `QuitDrain`, `drive_quit_drain`, the Quit/ContinueQuitDrain arms,
  and all their tests are untouched. C4 only ADDS a sibling variant + arms.
- `dispatch_save_then` itself: it is already action-agnostic (takes `PostSaveAction`).
- Swap lifecycle: save deletes the swap on success (save.rs:100) exactly as today; the
  discard path leaves it (decision 1) — no swap code changes at all.
- Scratch: never closable, prompt or not.

## Testing

**One sanctioned pin reversal (say it loud so no reviewer flags it):** workspace.rs's
`close_refuses_dirty_buffer` (:317) pins the refusal C4 exists to remove. It is REWRITTEN
as `close_dirty_raises_prompt` — dirty buffer + `close_buffer` → `editor.prompt.is_some()`
with the close-confirm choices, buffer still open, no status refusal. This is the only
existing test whose meaning changes.

**New unit tests:**
- prompt.rs: `close_confirm_routes_keys_case_insensitively` (the sibling idiom,
  prompt.rs:153-187).
- workspace.rs: `close_buffer_now_by_id_closes_inactive_buffer` (the by-id
  generalization — close a NON-active buffer's id; active buffer untouched);
  `close_buffer_now_vanished_id_is_noop_with_status`; the existing clean-path tests
  (:262-:315) must pass unchanged (they exercise `close_buffer` whose clean path now
  delegates).
- prompts.rs: `close_save_arms_pending_after_save_with_close_variant` (resolve
  `CloseSave` → `pending_after_save` carries `CloseBuffer { id }` with the right id and
  version); `close_discard_closes_immediately_and_leaves_swap` (a real swap file on disk
  survives — decision 1's pin); `close_save_on_unnamed_buffer_opens_save_as_with_carry`
  (`pending_save_as == Some(CloseBuffer { .. })`).
- jobs_apply.rs (the quit-arm test family's idioms — `quit_tmp`, `TestClock`,
  `InlineExecutor` + `apply_job_outcome` drain):
  `close_after_save_closes_on_matching_result` (buffer count drops, correct neighbor
  active); `close_cancelled_when_edited_during_flight` (buffer stays, status verbatim);
  `close_not_performed_on_save_failure` (symlink-target trick per
  `quit_drain_aborts_on_save_failure`); `close_result_for_wrong_buffer_is_stale_noop`
  (staleness keying); `close_after_save_last_ordinary_recheck` (arrange: two buffers,
  CloseSave on one, close the OTHER during flight via `close_buffer_now`, land the save
  → the last-ordinary path must fire at APPLY time, leaving a fresh untitled).
- app.rs (tick family): `close_save_timeout_cancels_with_status`.
- keymap: `ctrl_w_dispatches_close_buffer` (the binding exists and routes; the
  keymap test idiom).

**e2e journey** (e2e.rs Harness): dirty named buffer → `ctrl('w')` → prompt text on the
status row (`screen_contains`) → `key('s')` → save lands (InlineExecutor drain via the
harness's advance) → buffer closed, neighbor visible; and the `d` variant → buffer
closed, file on disk UNCHANGED, swap file still present.

**Gates:** the standard set — suite green (1,000 + the new tests), workspace clippy deny
clean, warning-free; smoke quoted verbatim pre-merge (advisory) + a live tmux sanity
(dirty buffer, ctrl-w, watch the prompt, press each of s/d/c across three runs).

## Non-goals (explicit)

- No change to quit behavior or its prompts; no drain for multi-buffer close (close is
  single-buffer by definition).
- No close-buffer mouse affordance (rides the overlay-mouse-parity follow-up).
- No wordstar-preset binding decision (D1/A5 territory).
- The `close_buffer` command stays scratch-refusing.
- No swap-lifecycle changes.

## Ship-time bookkeeping

Backlog: C4 → SHIPPED (note the ctrl-w binding and the swap-survives-discard
convention); working order advances (next = C2 transform scope). Memory: working-order
tick. Ledger: standard per-task lines.
