# Independent Fable follow-up review — R8 / R12 pre-merge cleanup delta

Reviewer: Claude Fable 5.1 (`claude-fable-5-1`), 2026-09-11.
Snapshot: `fix/quit-save-durability`, uncommitted against
`d3aea924988f60702529f6c68a0e332e64d44f1d`, verified against
`docs/reviews/evidence/2026-09-11-premerge-cleanup/source-sha256.json`
(26/26 files match before the review; `lib.rs` was temporarily edited to register
probes and restored to its manifest hash afterwards — see "Housekeeping").

Inputs read: `previous-fable-review.md`, `followup.diff`, `codex-review-round3.md`,
and the live source for every file the delta touches plus its callers
(`overlays.rs`, `editor.rs` open_* paths, `mouse.rs`, `file_browser_intercept.rs`,
`file_browser_commit.rs`, `prompts.rs`, `commands.rs`, `save.rs`, `jobs_apply.rs`,
`timers.rs`, `quit.rs`). Prior approval was treated as context, not proof.

## Verdicts

- **Design compliance: PASS.** Every delta item (M1, M2, M3, M6, M7, the two
  maintained regressions, the strengthened failure-wait assertion) is implemented
  as described and was verified against the real code and by probe.
- **Code quality: PASS with one Important finding.** The delta itself introduces no
  defect. The M1 fix is incomplete: it closes the overlay-registry route but not the
  mouse click-away route, which reaches the same orphaned state from ordinary user
  input. No data-loss, panic, lost-write, or unwanted-exit path was found.
- **Recommendation: NO-GO on this exact snapshot because of I1; GO once I1 is fixed
  and re-verified.** I1 is a one-line routing change plus a regression test. Nothing
  else blocks. Critical: 0. Important: 1 (new). Minor: 2 (new, both pre-existing
  code paths surfaced by the delta's own claim). Retained observations: 2.

## Findings

"New" means not reported by any prior review of this branch. "Pre-existing" means
the code path is on the baseline; I state whether the branch made it better, worse,
or unchanged.

### I1 (Important, new) — mouse click-away on a quit-owned picker still orphans the quit

- **Where:** `wordcartel/src/mouse.rs:536` (`mouse_file_browser`, `Down(Left)`
  outside the overlay rect: `editor.file_browser = None; // click-away closes`).
  This line bypasses the new `file_browser::close_overlay`
  (`file_browser.rs:402-406`), which the delta wired only into the overlay-registry
  row (`overlays.rs:142`).
- **Reachability (production path):** `app::reduce` → `Msg::Input(Event::Mouse)`
  (`app.rs:347-348`) → `mouse::handle` (`mouse.rs:767-792`, gated only on
  `mouse_capture`, default `true` at `editor.rs:783`) → `route_overlay`
  (`mouse.rs:82-89`) → `mouse_file_browser`. The picker's intercept passes
  non-key messages through (`file_browser_intercept.rs:150-151`). No plugin or
  second command is required: a mouse user clicking outside the dialog — the most
  ordinary "cancel" gesture — hits it.
- **Trigger:** Save and Quit (or Review Each → Save) on an unnamed document opens
  the quit-owned picker; click anywhere outside it.
- **Expected:** the prior review's M1 statement, which the delta set out to honor:
  leaving the picker by any route either keeps a live quit-owned request or cancels
  the quit as Esc does.
- **Actual:** `pending_save_as = Some(ContinueQuitDrain)` and `quit_drain` survive
  with no picker. Manual Save As (and Save on the unnamed document) is refused with
  "Save As is unavailable while quitting"; Save and Quit is refused with "another
  save or quit is in progress — try again". Recovery is Quit → summary → Cancel, or
  Esc on any prompt. No write is dispatched, no quit fires.
- **Impact:** confusing refusals from an ordinary gesture; strictly safer than the
  baseline (where the stale action would have fired a quit on the next manual Save
  As commit), but it defeats the purpose of the M1 fix for mouse users.
- **Evidence:** probes `c_mouse_click_away_on_quit_owned_picker_orphans_the_quit`
  (through `mouse::handle`; also records the refusals and the recovery path) and
  `c3_click_away_via_app_reduce_orphans_the_quit` (through `app::reduce`, the
  production message path). Contrast `c2_…manual_picker_is_a_plain_close`.
- **Fix shape (for the human to choose):** (a) minimal — route the click-away
  through `crate::file_browser::close_overlay(editor)` so it matches the registry
  row; or (b) mirror the keyboard Esc arm (`file_browser_intercept.rs:50-55`) and
  call `cancel_destination` for any Destination-mode picker, which would also close
  M3 below. (b) changes manual-picker closure behavior, which the delta deliberately
  kept as a plain close, so it is a design decision, not a silent fix.

### M2 (Minor, pre-existing) — the quit-owned overwrite prompt closed through the registry leaves the same orphan

- **Where:** `overlays.rs:145` (prompt row, `close: |e| e.prompt = None`). After a
  successful quit-owned commit onto an existing file, the picker is gone and the
  overwrite prompt carries the ownership (`file_browser_commit.rs:461-471`).
- **Trigger:** any `close_all` caller while that prompt is up (plugin-opened prompt
  or palette, `dispatch_overlay_command`, `Command::Quit` from a plugin).
  Keyboard cannot reach it (the prompt consumes keys) and the prompt's mouse slot
  only resolves choice cells, never closes on click-away (`mouse.rs:701-712`), so
  reachability is the same plugin-only class the prior review's M1 had.
- **Actual:** `pending_save_as = Some(ContinueQuitDrain)`, `quit_drain` Some,
  `pending_save_overwrite`/`pending_save_as_chosen` Some, no prompt. Same refusals
  and recovery as I1. Nothing is written; the target keeps its old bytes.
- **Evidence:** probe `d_close_all_over_quit_owned_overwrite_prompt_leaves_ownership_orphaned`.
- **Note:** not introduced by the delta and outside its stated scope, but it is the
  natural second half of M1 and the same `cancel`-based cleanup would close it.

### M3 (Minor, pre-existing on the baseline, outside R8/R12 scope) — mouse click-away on the close-buffer Save As picker leaves a stale CloseBuffer action

- **Where:** the same `mouse.rs:536` line. `cancel_destination` (Esc) clears
  `pending_save_as` for every destination picker (`file_browser.rs:384`); the mouse
  path clears nothing. Both lines predate the branch (`git show HEAD:` confirms).
- **Trigger:** close a dirty unnamed buffer → Save → picker opens with
  `pending_save_as = Some(CloseBuffer{id})` → click away → later, an unrelated
  manual Save As on that buffer.
- **Actual:** `perform_save_as` takes the stale action (`prompts.rs:207-210`); the
  save completes and then the buffer is closed with "saved — closed". The content
  is on disk at the chosen path first, so no data is lost, but the close is
  unexpected. This is exactly the stale-action class the branch closed for the quit
  flow (the "Critical-1, Task 21" comment at `file_browser_commit.rs:289-292`).
- **Evidence:** probe `c4_click_away_on_close_buffer_picker_leaves_stale_close_action`.
- **Disposition:** baseline behavior, unchanged by this branch; recommend a backlog
  item rather than widening this effort, unless fix shape (b) under I1 is chosen.

### Retained observations (prior review; unchanged, explicitly deferred by the brief)

- Prior M4 — a landed-but-unmerged manual save makes Save and Quit raise the
  external-mod modal (`save.rs:359-368`). Not re-probed; code unchanged.
- Prior M5 — the quit summary can be raised over a running Save All drain
  (`commands.rs:513-535`). Not re-probed; code unchanged.
- Prior M2's note that the `apply_result` ContinueQuitDrain failure branch
  (`jobs_apply.rs:49-53`) is reachable only with synthetic `save_request: None`
  requests still holds; it now calls `quit::cancel`, a safe superset.
- The bench harness discarding the barrier result (`e2e.rs` `step_timed`) is
  intentional per the brief. The checklist refresh was not re-audited here.

## Delta verification (claim → evidence)

| Delta item | Verified by |
| --- | --- |
| M1 — registry close of a quit-owned picker delegates to `close_overlay` → `cancel_destination`; non-owned closure unchanged | `overlays.rs:142`, `file_browser.rs:402-406`; maintained regression `replacing_quit_owned_picker_with_palette_cancels_its_ownership`; probe `a_…review_each_unnamed_picker_replaced_by_palette…` (second entry route) and `c2` (manual picker stays a plain close) |
| M1 — successful commit removes the picker before the overwrite prompt, so the transition stays valid | `file_browser_commit.rs:461` precedes `:471`; probe `b_quit_owned_commit_onto_existing_file_reaches_overwrite_and_exits` (ownership intact at the prompt, confirm → write → exit) |
| M1 — `Command::Quit` over a quit-owned picker now cancels via `close_overlay` before `quit::start`; Save All from the summary restarts cleanly with fresh ownership | `commands.rs:530` → `open_prompt` → `close_all`; `quit.rs:26-38`; probe `g_summary_over_quit_owned_picker_restarts_save_all_with_fresh_ownership`; maintained `summary_can_restart_a_quit_whose_filename_picker_it_replaced` |
| M1 — export redirect under a quit-owned picker still cancels first, then opens a non-owned picker | `file_browser_commit.rs:293-296` runs before `open_destination_picker`'s `close_all`; `quit::cancel` clears `fb.quit_save_owner` so `close_overlay` takes the plain path; maintained `quit_owned_export_redirect_cancels_the_quit` |
| M2 — failure/panic/timeout drain cancellations use `quit::cancel` | `jobs_apply.rs:52`, `:117`; `timers.rs:43` (and `:27`, already shared); probe `f_awaited_save_panic_in_save_all_cancels_without_stranding` (no strand, Save and Quit reusable) |
| M3 — `save_finished` returns whether it cancelled a plain-Quit wait; merge and panic statuses name it | `quit.rs:153-164`, `save.rs:275-277`, `jobs_apply.rs:109, 122-124`; maintained `changed_destination_explains_cancellation_of_a_plain_quit_wait` and the strengthened `quit_waiting_for_saves_times_out_or_cancels_on_failure`; probes `e_worker_panic_during_plain_quit_wait_names_the_cancelled_quit` (Error kind, both phrases), `e2` (no suffix when no quit waits, for both failure and panic), `e3` (a foreign failure neither cancels nor labels an awaited Save All; exit follows the awaited merge) |
| M3 — cancellation predicate unchanged | `waiting()` still requires an empty queue and no pending action/picker; the `save_finished` and `finish_save_action` cancel paths remain disjoint (`pending_after_save` None vs Some) |
| M6 — stale "fire a `Quit`" comment | `file_browser_commit.rs:292` now reads "trigger an unwanted quit" |
| M7 — guarded unwraps → `expect` | `jobs_apply.rs:189, 197` |
| Command-surface contract | no new command, option, binding, or hint; N/A |

## Checks actually run (this isolated checkout)

| Check | Result |
| --- | --- |
| Manifest `source-sha256.json` | 26/26 match before the review; `lib.rs` restored to its manifest hash after (all other sources untouched) |
| `cargo test --workspace` | 2,498 passed, 0 failed, 6 ignored across 18 summaries (reproduces the brief's claim) |
| Focused durability suite | 32 passed (includes both new regressions) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo build --workspace` | warning-free |
| `cargo test --workspace --no-run` | warning-free |
| `scripts/smoke/run.sh` | `smoke: 9/9 PASS` (run here, private tmux server) |
| Fable probes (12) | 12 passed (`review-probes/fable_followup_probes.rs`) |

One probe (`e_…`) failed on its first run because of a probe-authoring error (a
clean named document's plain Save skips the unchanged write, so no job was queued);
it was rewritten to use a Save As dispatch like the maintained wait tests and passes.
No product defect was involved.

## Limitations

- I1 was driven through `app::reduce` with a synthetic mouse event, not through a
  real terminal; the smoke suite does not cover mouse input.
- M2's plugin-callback reachability is reasoned from source (the registry close row
  and the pump's dispatch path), not driven through Lua.
- Prior M4/M5 were not re-probed; their code is unchanged by the delta.
- The checklist, implementation report, and plan documents were not audited.
- External-writer races, symlink retargeting, and recovery cadence remain out of
  scope.

## Housekeeping

No production source was changed. `lib.rs` carried a temporary
`#[cfg(test)] #[path = "../../review-probes/fable_followup_probes.rs"]` line while
the probes ran and was restored; its hash matches the manifest. Additions are
`review-probes/fable_followup_probes.rs` and this report. No rustfmt, commit, push,
merge, delegation, credential access, or external contact occurred. The smoke run
may have appended to the gitignored `scripts/smoke/.history`.
