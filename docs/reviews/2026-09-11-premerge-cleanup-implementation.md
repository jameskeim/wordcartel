# R8 / R12 pre-merge cleanup

Reviewed branch: `fix/quit-save-durability`, based on
`d3aea924988f60702529f6c68a0e332e64d44f1d`.
Implementation committed as `fbb2a1ca6a5dfd3403b7dda2d2e5d22d34bf4c93`; local merge authorized on 2026-09-11.
This supersedes the earlier R8/R12 implementation snapshot for final review.
Tracking: [checklist](2026-09-11-r8-r12-premerge-checklist.md).

## Implemented efforts

1. Removed the unreachable legacy quit action, prompt, actions, and timeout chain.
   Rendering/mouse fixtures now exercise the live multi-document prompt. Applicable
   safety assertions were preserved on the live flow; command IDs and bindings stay intact.
2. `SaveRequest` is an immutable identity. Its completion flag lives on the waiting
   `PendingAfterSave`, eliminating the stateful Copy value.
3. Jobs and panic outcomes carry save-request identity. Inline and threaded executors
   share the same `Job::execute` panic boundary. A foreign same-version panic cannot
   cancel another request; the awaited request's own panic still cancels safely.
4. Destination Esc, empty submission, and export redirect use shared quit cleanup.
   Cancellation revokes provisional exit, preserves an unrelated pending close, and
   clears quit ownership on any picker retained for a manual retry.
5. Documented request/discard/review/dispatch state. Kept the existing safe policy:
   ordinary Quit supersedes a pending close; Save and Quit refuses to replace it.
   The earlier review's personal-state-directory claim remains corrected.
6. Implemented the user-approved Save As policy. New manual operations are blocked
   during quit, including submission of an older picker and a late overwrite prompt.
   Quit-owned filename requests remain available and are bound to the correct buffer.
   Tracked saves must finish and merge, including path/session migrations, before exit.
   An ordinary Quit waiting for existing saves asks for review if new edits arrive;
   it does not implicitly authorize saving them. Failure or a five-second timeout
   cancels that wait. A confirmed summary choice can restart a quit whose picker it replaced.
7. Retained the conservative warning/cancellation when an already-queued old-path
   save follows Save As. A regression covers the clean-new-path case requiring another quit.

## Independent review correction

The first independent Codex pass found one Important runtime gap: reduce could
decide to quit before a queued plugin command ran, and the runtime used that stale
decision after the plugin pump. A real-plugin regression reproduced the lost edit.

The runtime and end-to-end harness now share `app::finish_iteration`, which checks
exit after callbacks. Quit remains provisional until then; discard decisions stay
available for the final rescan. New edits/saves keep the app running, manual Save As
stays blocked, and cancellation revokes exit. The final loop break uses this decision.

- [Initial NO-GO report](evidence/2026-09-11-premerge-cleanup/codex-review.md)
- [Failing plugin reproduction](evidence/2026-09-11-premerge-cleanup/post-pump-red.log)
- [Two passing real-plugin regressions](evidence/2026-09-11-premerge-cleanup/post-pump-tests.log)
- [Independent Codex round two: GO, no findings](evidence/2026-09-11-premerge-cleanup/codex-review-round2.md)

## Fable follow-up corrections

The full cleanup review returned GO with Minor observations. We fixed quit-owned
picker closure through the overlay registry, consolidated remaining cancellation
sites, clarified failure/panic status when a plain Quit wait is cancelled, and
addressed the small comment/guarded-unwrap observations. Maintained regressions
cover picker replacement and cancellation wording.

The follow-up found one Important omission: mouse click-away bypassed that picker
cleanup. The mouse route now calls `file_browser::close_overlay`; a regression
through the actual `app::reduce` mouse-input path failed before the change and
passes after it. Non-quit picker behavior remains unchanged.

- [Full cleanup review](evidence/2026-09-11-premerge-cleanup/fable-round1/fable-review.md)
- [Follow-up finding I1](evidence/2026-09-11-premerge-cleanup/fable-round2/fable-review.md)
- [Mouse reproduction](evidence/2026-09-11-premerge-cleanup/mouse-red.log) and
  [passing regression](evidence/2026-09-11-premerge-cleanup/mouse-green.log)
- [Final independent Codex review: GO](evidence/2026-09-11-premerge-cleanup/codex-review-round4.md)

The final Fable verification returned **GO**, with I1 resolved and no Critical or
Important findings. Its five independent mouse probes passed; it independently
reproduced the 2,499-pass workspace result and 9/9 smoke result.
[Final Fable report](evidence/2026-09-11-premerge-cleanup/fable-review.md).

## Explicit follow-ups

These pre-existing, nonblocking observations remain deferred; this change does not
claim to resolve them:

- Fable follow-up M2: a plugin replacing a quit-owned overwrite prompt can leave
  pending quit ownership without a prompt. Quit → summary → Cancel recovers it;
  no write or exit occurs. Track shared prompt cancellation separately.
- Fable follow-up M3: mouse click-away on a close-buffer Save As picker can retain
  the pending close, causing a later manual Save As to save and then close the
  buffer. Track non-quit destination cancellation separately.
- Full cleanup M4: a manual save written but not yet merged can cause an extra
  external-modification prompt during Save and Quit.
- Full cleanup M5: a quit summary can appear while an existing Save All continues.

- Final Fable N1: the click-away rectangle uses entry count while the painted
  picker includes reserved footer rows. A bottom-border/footer click can therefore
  count as outside. This predates the branch; cancellation is now safe. Defer
  unifying hit-test and painter geometry to a focused UI follow-up.
- Final Fable N2: the mouse-handler doc comment says click-away closes the browser
  without naming quit cancellation. The changed inline comment explains it; defer
  this cosmetic clarification to the next edit of that file.

The timed benchmark intentionally returns timing data; final quit state remains on
Editor. The earlier checklist-staleness observation described the isolated review
packet, not the subsequently updated live checklist. The original unrelated
review findings (R1–R7 and R9–R11) remain separate work.

## Validation

- Thirty-three focused durability regressions pass, including both executors, picker and
  overwrite ownership, already-queued save merges, final discard/cancel decisions,
  and the retained warning.
  The earlier Fable probes are promoted into maintained regressions: P1 maps to
  `quit_waits_for_existing_save_as_merges_and_session_migrations`; P4 to
  `old_manual_picker_cannot_submit_and_esc_cancels_awaited_quit`; P9 to
  `save_as_before_quit_retains_the_conservative_warning`; and P11 to
  `panic_cancels_only_its_own_save_request_in_both_executors`.
- Two additional real-plugin end-to-end tests pass: post-save edits and newly queued
  saves cannot evade the final exit check; late manual Save As is blocked.
- Workspace suite: **2,499 passed, 0 failed, 6 ignored**, across eighteen summaries.
- Workspace clippy, workspace build, and test compilation: passed, warning-free.
- Terminal smoke: **`smoke: 9/9 PASS`**. Final Codex and Fable reviews both returned
  GO; all 26 source hashes still match the reviewed snapshot.

Logs and the final patch/hash manifest are under
`docs/reviews/evidence/2026-09-11-premerge-cleanup/`. Tests use isolated state and
temporary documents; unrestricted execution is needed only for socket/tmux fixtures.
No rustfmt or push was performed. Original unrelated scratchpad material remains
untouched. Other durability-review findings remain outside this effort.

## Authorized integration

The local `--no-ff` merge into `main` applied without conflicts. Before finalizing
the merge commit, `cargo test --workspace` on the merged tree passed: **2,499 passed,
0 failed, 6 ignored** across eighteen summaries. All 26 source hashes match the
reviewed snapshot. [Merged-tree test log](evidence/2026-09-11-premerge-cleanup/merged-workspace-tests.log).
Raw CLI event streams and launcher metadata remain local working artifacts; the
commit retains reports, probes, source manifests, and validation logs.

Commit metadata retains the requested project credit and marks `Claude-Session` as
unavailable: the Codex harness supplied no Claude session URL, and none was invented.
