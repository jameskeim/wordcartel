# Independent Fable re-review — R8 / R12 pre-merge cleanup

Reviewer: Claude Fable 5.1 (`claude-fable-5-1`).
Snapshot: `fix/quit-save-durability`, uncommitted against
`d3aea924988f60702529f6c68a0e332e64d44f1d`, verified against
`docs/reviews/evidence/2026-09-11-premerge-cleanup/source-sha256.json`
(25/25 files match, checked before and after the review; `lib.rs` was temporarily
edited to register probes and restored to its manifest hash).

## Verdicts

- **Design compliance: PASS.** Every user-approved item in the design's
  "Approved pre-merge follow-ups" section is implemented as described, and the
  post-callback exit barrier is placed correctly in both the real run loop and
  the end-to-end harness.
- **Code quality: PASS with Minor findings.** No data-loss, panic, stranded-flow,
  or lost-write path was found. Findings are consistency, UX-wording, and
  documentation items.
- **Recommendation: GO.** Critical: 0. Important: 0. Minor: 7 (three new,
  two pre-existing limitations, two documentation/style).

## Findings

Severity is my own classification. "New" means introduced or left incomplete by
this branch; "pre-existing" means present on the baseline and not made worse.

### M1 (new, cleanup gap) — a quit-owned picker closed by `close_all` orphans quit ownership

- **Where:** `wordcartel/src/overlays.rs:141-142` (file-browser row,
  `close: |e| e.file_browser = None`) versus `file_browser::cancel_destination`
  (`file_browser.rs:382-397`), which is only reached from the picker's own Esc.
  `close_all` callers: `Editor::open_prompt` / `open_palette` (`editor.rs:939-953`),
  `app::dispatch_overlay_command` (`app.rs:234`), and the plugin
  `dispatch_with_arg` arm under an open overlay.
- **Trigger:** Save and Quit (or Review Each → Save) on an unnamed document opens the
  quit-owned picker; anything that raises a prompt/palette while it is up closes it
  through the overlay row. Keyboard input cannot do this (the picker consumes keys),
  but plugin callbacks can run under an open picker
  (`file_browser_commit.rs:478-480` documents exactly that), and `Command::Quit`
  itself does it (the branch's own test `summary_can_restart_a_quit_whose_filename_picker_it_replaced`).
- **Expected:** leaving the picker by any route either keeps a live quit-owned
  request or cancels the quit as Esc does.
- **Actual:** `pending_save_as = Some(ContinueQuitDrain)` and `quit_drain` survive
  with no picker. Manual Save / Save As are refused with "Save As is unavailable
  while quitting"; Save and Quit is refused as busy. Recovery is Quit → summary →
  Cancel (or Save All), or Esc on any prompt.
- **Impact:** confusing refusals; no write is lost and no quit fires. This is
  strictly safer than the baseline, where the stale `pending_save_as` would have
  fired a quit on the next manual Save As commit (the loophole effort 6 closed).
- **Evidence:** probe `e_orphaned_quit_owned_picker_after_close_all_blocks_manual_save_until_summary`
  (passes; records the observed refusal and recovery).

### M2 (new, consistency) — three sites still clear drain state without `quit::cancel`

- **Where:** `jobs_apply.rs:116-120` (`apply_panic`, awaited-request branch),
  `jobs_apply.rs:50-54` (`apply_result`, ContinueQuitDrain `!saved_this` branch),
  `timers.rs:40-47` (`save_timeout_tick`, ContinueQuitDrain timeout).
- **Trigger / actual:** each sets `quit_drain = None; quit_drain_advance = false`
  directly. Effort 4 states destination cancellations share the quit cleanup
  helper; these failure/timeout sites do not.
- **Impact today:** none observable. `editor.quit` cannot be true at these points,
  and no picker can be open while `pending_after_save` is Some. The `apply_result`
  branch is unreachable with production requests because `finish_save_action`
  already clears the pending action and cancels on failure (only synthetic
  `save_request: None` tests reach it). The risk is drift: a future field added to
  `cancel` (like `quit_save_owner`) will be missed here.
- **Evidence:** source reading; probe `p_awaited_save_panic_on_thread_executor_cancels_quit_and_worker_survives`
  confirms the panic path still reaches a consistent end state on the real worker.

### M3 (new, UX wording) — a foreign failure during the plain-Quit wait cancels the quit silently

- **Where:** `quit.rs:154-161` (`save_finished`).
- **Trigger:** clean workspace, Quit while already-dispatched saves are in flight
  (the new waiting phase); one tracked save then fails, or completes as a
  changed-destination write (queued Save As followed by an old-path save).
- **Expected:** the design approves conservative cancellation; the timeout path
  says "save timed out — quit cancelled", so a cancellation here should also name
  the cancelled quit.
- **Actual:** `cancel` runs with no status of its own; the only visible text is the
  save error or "Saved to … — current document was not saved". In the
  changed-destination case the current path is already clean, so the user must
  simply press Quit again.
- **Impact:** momentary confusion; no data loss.
- **Evidence:** probe `l_changed_destination_during_plain_quit_wait_cancels_quit_with_only_the_warning`.
  Contrast probe `l2_foreign_changed_destination_does_not_cancel_awaited_later_save`,
  which confirms the intended protection: a foreign old-path warning does not
  cancel a Save All that awaits its own later request, and exit follows that merge.

### M4 (pre-existing) — a landed-but-unmerged manual save makes Save and Quit raise the external-mod modal

- **Where:** `save.rs:358-367` (`dispatch_save_reporting` fingerprint check
  against `stored_fp`).
- **Trigger:** Ctrl+S, then Save and Quit before the worker's result has merged
  on the foreground (window: worker write → next drain). Same window applies when
  Save All is chosen from a summary raised over a running drain (`quit::start`
  cancels and refills while the previous write may have landed).
- **Actual:** conflict prompt, quit cancelled cleanly (drain None, no pending).
- **Impact:** an extra prompt; safe. The check predates this branch.
- **Evidence:** probe `f_preexisting_landed_but_unmerged_manual_save_makes_save_and_quit_conflict`.

### M5 (pre-existing) — the quit summary can be raised over a running Save All drain

- **Where:** `commands.rs:513-536` (`Command::Quit`), no modal is up while a
  drain's save is running, so Ctrl+Q is reachable.
- **Actual:** the drain keeps advancing under the summary (merges are applied by
  `prompts::intercept`), can exit with the summary open, or a Save All choice
  restarts and issues a redundant same-version write. Cancel leaves no stranded
  state and already-dispatched writes land.
- **Impact:** cosmetic. Same shape on the baseline.
- **Evidence:** probe `o_quit_summary_during_save_all_wait_then_cancel_leaves_no_stranded_state`.

### M6 (documentation drift)

- `file_browser_commit.rs:292` still says "fire a `Quit` the writer no longer
  wants"; `PostSaveAction::Quit` was removed by this branch.
- `docs/reviews/2026-09-11-r8-r12-premerge-checklist.md` leaves every effort item
  unchecked and has a single completion-log row, while the implementation report
  states all seven efforts are complete. The checklist's own rule says an unchecked
  item remains outstanding unless explicitly deferred. Refresh before merge.

### M7 (style)

- `jobs_apply.rs:190` and `:198` add two guarded `.unwrap()`s on `quit_drain`
  (following the function's pre-existing pattern) where the house style prefers
  `expect("…invariant…")`.
- `e2e.rs:199` (`step_timed`) binds the barrier result as `_keep`; fine for a
  bench helper, but it means the bench harness cannot observe exit.

## Design-compliance verification (claim → evidence)

| Approved item | Verified by |
| --- | --- |
| Save and Quit saves all ordinary unsaved documents; active doc's explicit-save step preserved | `quit::save_and_quit` seeds `[active]` then `refill` (quit.rs:18-23, 46-55); tests `save_and_quit_does_not_exit_with_other_dirty_buffers`, probes C, H (clean named doc still dispatches its save) |
| Legacy `PostSaveAction::Quit` / `quit_confirm` / `QuitAnyway` / `SaveAndQuit` removed; `save_and_quit` command kept | grep: no remaining references except the M6 comment; `registry.rs:302-306` |
| Immutable `SaveRequest`; completion on the pending action | `save.rs:73-83`, `editor.rs:52-58`, `finish_save_action` (save.rs:299-310), `apply_result` fire predicate (jobs_apply.rs:27-30); probe L2 |
| Request identity on panic outcomes, both executors | `jobs.rs:57-76` (`Job::execute` shared boundary), `apply_panic` (jobs_apply.rs:109-127); test `panic_cancels_only_its_own_save_request_in_both_executors`; probe P on the real worker |
| Shared cancellation for destination Esc / empty / export redirect | `file_browser.rs:396`, `file_browser_commit.rs:295, 380`; tests `old_manual_picker_cannot_submit…`, `quit_owned_export_redirect_cancels_the_quit`; residual gaps are M1/M2 |
| Manual Save As blocked at open, submission, and write; quit-owned requests allowed and bound to the right buffer | `quit.rs:106-141`, `prompts.rs:84-108`, `file_browser_commit.rs:352-356`, `prompts.rs:196`; tests `manual_save_as_is_blocked…`, `quit_filename_and_overwrite_confirmation_reject_changed_buffer`; probes B, C, D (Review Each unnamed, second unnamed buffer, quit-owned overwrite confirm) |
| Already-dispatched saves merge before exit; session migrations preserved; 5 s timeout on the wait | `drive_quit_drain` (jobs_apply.rs:188-196), `wait_for_saves`, `sq_deadline` (timers.rs:91-95); tests `quit_waits_for_existing_save_as_merges_and_session_migrations`, `quit_waiting_for_saves_times_out_or_cancels_on_failure`; probe G (wake armed at `since + 5001`, disarmed after cancel, no idle spin) |
| Conservative destination-mismatch warning/cancellation retained | `save.rs:191-199, 275-276`; test `save_as_before_quit_retains_the_conservative_warning`; probe L |
| Ordinary Quit supersedes a pending close; Save and Quit refuses when busy | `commands.rs:518-524`, `quit::busy`; tests `overlapping_quit_requests_do_not_replace_pending_work`, workspace busy-guard tests |
| Post-callback exit barrier: provisional pre-pump quit, discard decisions retained, live dirty / in-flight rechecked, cancellation honored, Save As still blocked | `app.rs:852-853` (reduce result discarded), `:858` pump, `:902` `finish_iteration`, `:928` break on that result; harness parity `e2e.rs:152-172`; `quit::after_callbacks` (quit.rs:173-182); `in_progress` includes `editor.quit`; e2e plugin tests; probes A (plain Quit → late edit → review prompt), J (discards retained, new buffer rescanned once), K (SaveAll mode retained across the barrier) |
| Command-surface contract | no new command/option/binding; `save_and_quit` and `quit` IDs and registration unchanged |

Stages after the barrier (`advance`, render, session persistence) were read and
dispatch no document save and run no plugin callback, matching the Codex round-two
statement.

## Checks actually run (this isolated copy)

| Check | Result |
| --- | --- |
| Manifest `source-sha256.json` | 25/25 match, before and after the review |
| `cargo test --workspace` | 2,496 passed, 0 failed, 6 ignored, across 18 summaries (independently reproduces the claim) |
| Focused durability suite (30) + real-plugin e2e (2) | included in the 2,036-test `wordcartel` lib run above; all pass |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo build --workspace` | warning-free |
| `cargo test --workspace --no-run` | warning-free |
| Fable probes (16) | 16 passed |
| `scripts/smoke/run.sh` | **not run here** — the session's permission mode denied spawning it. Supplied log states `smoke: 9/9 PASS` (unverified by me) |

Two probes failed on their first run because of probe-authoring errors (a dirty
document where the wait phase requires a clean one; an expectation that a foreign
completion cancels an awaited later request, which the design explicitly protects).
Both were corrected and the corrected expectations are recorded above; neither
revealed a product defect.

## Probe sources

`review-probes/fable_probes.rs` (kept; registered during the run via a temporary
`#[path]` line in `wordcartel/src/lib.rs`, since removed). Probes A–P cover:
plain-Quit provisional exit with a late edit; Review Each on an unnamed buffer;
Save All with a second unnamed buffer; quit-owned overwrite confirm; orphaned
picker ownership (M1); landed-but-unmerged save conflict (M4); timeout wake
arming/disarming; clean-named Save and Quit; repeated Quit while waiting;
discard retention plus new-buffer rescan; SaveAll mode retained by the barrier;
changed-destination during the plain wait (M3) and against an awaited later
request; real `ThreadExecutor` Save All end to end; summary over a running drain
(M5); awaited-save panic on the real worker.

## Practical coverage limits

- The smoke suite was not executed here; the real `wcartel` binary was not driven.
- Real-loop placement of the barrier was verified by reading `app::run` and by
  harness parity, not by running `app::run` (needs a terminal). The real
  `ThreadExecutor` was exercised with a hand-driven drain loop.
- M1's plugin-callback reachability is reasoned from source (the write-block
  comment and the pump's dispatch path), not driven through Lua.
- External-writer races, symlink retargeting, recovery identity/cadence, and the
  implementation plan document were out of scope and not audited.

## Housekeeping

No production source was changed; `lib.rs` matches its manifest hash. The only
additions are `review-probes/fable_probes.rs` and this report. No rustfmt, commit,
push, merge, delegation, or external contact occurred.
