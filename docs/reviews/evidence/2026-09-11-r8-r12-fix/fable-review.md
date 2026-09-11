# Fable whole-branch review — R8 / R12 save completion and quit safety

Branch: `fix/quit-save-durability` (uncommitted working tree) against base
`d3aea924988f60702529f6c68a0e332e64d44f1d`. Reviewer: Claude Fable 5.1, 2026-09-11.
Reviewed tree verified: every file in
`docs/reviews/evidence/2026-09-11-r8-r12-fix/source-sha256.json` hashes to the recorded value
in this checkout (`sha256sum` on all eleven files; `lib.rs` re-verified after the temporary
probe registration was reverted).

## 1. Verdicts

**Design compliance: PASS.** Every requirement in Sections A, B and C of the design and in
plan sections 2–4 is implemented as written, with one deliberate design consequence to put in
front of the human (Finding 3: the retained legacy `Quit` chain is now unreachable from
production code). Command-surface conformance holds: no registry, keymap, menu, palette, or
option change; `save_and_quit` keeps its ID and reaches `quit::save_and_quit` from every
surface via the single registered handler.

**Code quality: PASS with Minor findings.** No Critical or Important findings. The new code
matches the house style, keeps the dispatch hubs thin (`quit.rs` owns the orchestration;
`drive_quit_drain` and `apply_result` gained delegations, not bodies), and introduces no
unwrap on a fallible path. The Minor items are: a request-identity gap on the panic path, three
duplicated cancel sites that predate the branch but now sit beside a helper that supersedes
them, a `Copy` value type carrying a mutable flag, and missing doc comments on new fields.

**Merge recommendation: GO**, conditional on the human deciding Finding 3 (keep the dead
legacy chain as the spec says, or drop it in a follow-up). Nothing else blocks. The Minor
findings can go to the ledger.

## 2. Findings, ordered by impact

No Critical findings. No Important findings.

### Finding 1 — Minor (new gap): the panic path ignores `SaveRequest` identity

- **Where:** `wordcartel/src/jobs_apply.rs`, `apply_panic`, `JobKind::Save` arm (lines
  124–130): `awaited = p.buffer_id == buffer_id && p.version == version`.
- **Trigger:** a Save job for the same buffer and version as the awaited request panics
  while a different request is the one awaited (e.g. Save and Quit armed on request N, an
  unrelated same-version save panics).
- **Expected vs actual:** the design's Section C refinement says only the exact awaited
  request may complete or cancel its action. Actual: the panic clears `pending_after_save` and
  the drain regardless of request id, so the awaited flow is cancelled by a stranger.
- **Impact:** safe direction only — the editor never quits or closes on this path; the user's
  quit silently disappears and the panic's Error status is shown. No data loss.
- **Evidence:** probe P11 (`review-probes/review_probes_fable.rs`) prints
  `drain=false pending=false` after the foreign panic; the awaited save then lands with
  `quit=false`.

### Finding 2 — Minor (pre-existing, now inconsistent): three cancel sites bypass `quit::cancel`

- **Where:** `wordcartel/src/file_browser.rs` `cancel_destination` (lines 394–397);
  `wordcartel/src/file_browser_commit.rs` Redirect arm (293–297) and `CommitOutcome::Nothing`
  arm (373–377). Each clears `pending_save_as` and `quit_drain` by hand and leaves
  `pending_after_save` alone.
- **Trigger:** a Save and Quit save is in flight (`pending_after_save = ContinueQuitDrain`),
  the user opens a manual Save As picker (no busy guard on `save_as`) and backs out with Esc.
- **Expected vs actual:** the branch's `quit::cancel` clears the drain *and* the awaited
  quit action together. These three sites clear only the drain, so the armed action survives;
  when the save lands the `ContinueQuitDrain` arm fires, finds no drain, and the quit
  evaporates with status `Saved` and no explanation.
- **Impact:** no data loss and no strand (the pending is consumed by its own completion).
  UX only: a "silent" non-quit. This behaviour predates the branch; the branch added the
  helper that makes the duplication visible. Routing all three through `quit::cancel` would
  be a one-line change each.
- **Evidence:** probe P4 prints `after Esc: drain=false pending=true`, then
  `after completion: quit=false status="Saved"`.

### Finding 3 — Minor (design decision for the human): the legacy `Quit` chain is dead in production

- **Where:** `PostSaveAction::Quit` (`editor.rs`), its arm in `apply_result`
  (`jobs_apply.rs:35–47`), the `Quit` arm of `timers::save_timeout_tick` (`timers.rs:33–38`),
  `Prompt::quit_confirm` (`prompt.rs:110`), and `PromptAction::SaveAndQuit` /
  `PromptAction::QuitAnyway` (`prompts.rs:254–260`).
- **Trigger:** none. `dispatch_save_and_quit` now delegates to `quit::save_and_quit`, which
  arms `ContinueQuitDrain`; `dispatch_save_then(Quit)` has no production caller (grep: only
  `durability_regressions.rs` and `save.rs` tests). `quit_confirm` was previously reachable
  only via the `Quit` timeout re-raise, so the whole chain is now test-only.
- **Expected vs actual:** the spec and plan say "retain the legacy `PostSaveAction::Quit`
  variant but guard its exit against the full dirty set too"; the implementation does exactly
  that (and the guard is correct — `legacy_quit_action_cannot_bypass_the_other_dirty_document`
  covers it). The house rule says no dead code. Both are satisfied only if the human accepts
  retained-but-unreachable code as a deliberate choice.
- **Impact:** none at runtime. Maintenance: a variant that must be kept exhaustive in three
  matches and a prompt that can no longer appear.
- **Recommendation:** decide explicitly; if dropped, it is a self-contained follow-up
  (remove the variant, the `quit_confirm` prompt, two prompt actions, and their tests).

### Finding 4 — Minor (pre-existing): quit can be set while a later Save As job is still queued

- **Where:** `quit::refill` / `drive_quit_drain` exit decision; the `save_as` command has no
  busy guard.
- **Trigger:** Save and Quit dispatches request N (ordinary save to A); before it lands the
  user performs Save As to B (request N+1). FIFO: N lands, A is clean, refill finds nothing
  dirty, `quit = true` while N+1 is still queued.
- **Expected vs actual:** the run loop joins the worker on exit (`app.rs:942–947`), so the
  bytes for B do land. The Save As *merge* never runs: no rekey, no session migration, the
  session entry is persisted under A. No document content is lost.
- **Impact:** bookkeeping only; identical to the baseline `Quit` arm's behaviour. Out of
  scope for R8/R12 but worth a ledger note next to the `save_as` busy-guard question.
- **Evidence:** probe P1 prints `quit=true pending_jobs=1 path=…/a.md`.

### Finding 5 — Minor (UX, spec-consistent): Save As followed by Save and Quit cancels the quit with a Warning

- **Where:** `save::finish_save_action` failure branch (destination mismatch).
- **Trigger:** Save As A→B dispatched, then Save and Quit (not busy — a plain Save As arms
  nothing). The ordinary save targets A, lands second, mismatches B, and the awaited quit is
  cancelled with `Saved to …/a.md — current document was not saved` even though the buffer
  is clean at B.
- **Expected vs actual:** matches Section C and plan section 2 exactly ("cancel a matching
  awaited post-save action on … destination mismatch"). Recorded because the user sees a
  Warning and no quit in a state where nothing is unsaved; they must re-issue quit.
- **Impact:** no data loss, no strand (`quit_drain` and `pending_after_save` both `None`).
- **Evidence:** probe P9.

### Finding 6 — Minor (code quality): `SaveRequest` is `Copy` with a mutable `completed` flag

- **Where:** `save.rs:71–82`; mutated only through `pending.save_request.as_mut()` in
  `finish_save_action`; the closure's copy is never marked.
- **Observation:** correct as written, but "which copy is authoritative" is implicit. A
  `completed: bool` on `PendingAfterSave` (or an `Option<u64>` id plus a separate flag) would
  say the same thing without a stateful value type. No behavioural issue.

### Finding 7 — Minor (docs/style)

- `QuitDrain::discarded_versions` and `QuitDrain::reviewing` (`editor.rs:32–33`) have no doc
  comments; `SaveDispatch` variants (`save.rs:336`) are undocumented individually.
- `finish_save_action`'s failure branch calls `quit::cancel` for every action kind including
  `CloseBuffer`; harmless today (the busy guards keep a drain and a close from coexisting) but
  the coupling is not stated in the comment.
- `quit::save_and_quit` refuses when a `CloseBuffer` pending is armed (busy), while
  `Command::Quit` supersedes and clears a pending close. Safe, but the two quit entries now
  answer "what about a pending close" differently.
- `durability_regressions::late_save_to_old_path_does_not_mark_current_path_clean` writes a
  checkpoint through `swap::write_atomic` into the real state dir (same pattern as existing
  swap tests; relies on the launcher's XDG isolation).

## 3. What was checked

### Claims verified against source

- Section A: `quit::save_and_quit` seeds the drain with the active id and calls
  `dispatch_save_then(ContinueQuitDrain)`, preserving the explicit Save/Save As step (probe P5
  shows a clean named active is still saved first; `save_and_quit_preserves_clean_unnamed_save_as_and_cancel`
  covers the unnamed case). Registered command and `PromptAction::SaveAndQuit` both reach it.
- Section B: `QuitDrain::new` starts with empty decisions; `show_review` pins
  `(id, version)`; `review_discard` records that pair and pops only if it is at the front;
  `review_save` re-displays a stale review and switches to the reviewed buffer, not the
  active one; `refill` excludes only explicit same-version discards; `quit::cancel` drops all
  decisions. `Document.version` is strictly monotonic (`editor.rs:357/388/412` — apply, undo,
  redo; `reload_from_disk` continues from `previous_version + 1`), so version-keyed discards
  cannot be resurrected by undo (probe P13). `alloc_id` never reuses ids.
- Section B dispatch audit: `SaveDispatch { Queued, Picker, Conflict, Rejected }` replaces
  the boolean; `Conflict`/`Rejected` cancel a quit action and preserve the conflict modal
  (`conflict_cancels_quit_without_stranding_the_drain`, probe P3 for the Review Each path).
- Section C: the `changed_destination` guard applies to `SaveMode::Normal` only, leaves
  `saved_version`, `stored_fp`, `swapped_version`, swap files and migrations untouched, finishes
  the topic as Warning, and still fires the Save plugin event for the path actually written.
  Save As rekey remains FIFO (`sequential_save_as_and_same_path_stale_saves_remain_valid`).
  `fire_event` only enqueues (`plugin/mod.rs:120`), so no plugin re-entry during the merge.
- Request identity: `finish_save_action` filters on `(id, version, request.id)` and marks
  completion; `apply_result` requires `completed` before firing. `perform_save_as` and
  `dispatch_save_then` are the only producers and both bind a real request.
- Production ordering: results reach `apply_job_outcome` via `Msg::JobDone` in `reduce` and
  under a modal prompt (`prompts::intercept`), and via `fold_and_continue`; the file-browser
  intercept passes non-key messages through. `pre_recv` runs the timeout guard before each
  blocking receive; `sq_deadline` wakes the loop at `at_ms + 5 s`.
- Callers of the changed state: `close_buffer` busy guard (probe P15), `Command::Quit`
  re-entry (probes P7, P8), timeout mid-drain (P10), unnamed second buffer under Save All
  (P2), edit during own save (P6), Review Save write failure (P14).

### Commands run and results

| Command | Result |
| --- | --- |
| `sha256sum` on the 11 manifest files | all match `source-sha256.json` |
| `cargo test -p wordcartel --lib durability_regressions` | 16 passed, 0 failed |
| `cargo test -p wordcartel --lib review_probes_fable -- --nocapture --test-threads=1` (probe module temporarily registered) | 15 passed, 0 failed |
| `cargo test -p wordcartel` (lib + all integration suites: backlog, module budgets, command surface, fs seam) | 10 summaries, 2064 passed, 0 failed, 6 ignored |
| `git diff HEAD` over all modified files | read in full |

### Tool limits

- `cargo clippy --workspace --all-targets` was denied by the sandbox (twice). The evidence
  `clippy.log` shows only a warm-cache `Checking wordcartel … Finished` line; it does not
  itself show the `--all-targets` invocation. I could not independently reproduce that gate.
- `cargo build` and `cargo test --no-run` were not re-run separately; the `cargo test -p
  wordcartel` compile emitted no warnings in its output.
- The PTY smoke suite was not re-run (evidence: `smoke: 9/9 PASS`).
- `rm` / `git clean` were denied. The temporary module file
  `wordcartel/src/review_probes_fable.rs` remains in the tree as a three-line comment stub;
  it is **not** registered in `lib.rs` and is not compiled. Delete it before commit.
  `lib.rs` was restored and re-hashed to the manifest value.

### Preserved probes

`review-probes/review_probes_fable.rs` — 15 tests (P1–P15) with run instructions in the
header. P1, P4, P9, P11 print the observed state that Findings 1, 2, 4 and 5 cite.

### Coverage limits

- Deterministic in-process executor only; no real-thread interleaving of the worker was
  exercised beyond what FIFO delivery models. Production ordering was checked by reading the
  loop, not by driving `ThreadExecutor`.
- Plugin hooks were checked only for re-entrancy (none: events are queued). Plugin-driven
  edits between a merge and the next drive step were not simulated.
- Symlinked destinations and the external-writer race (R7) are out of scope per the design
  and were not probed.
