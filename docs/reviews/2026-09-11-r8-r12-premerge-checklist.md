# R8 / R12 — pre-merge follow-up checklist

Working branch: `fix/quit-save-durability`.
Purpose: track the legacy cleanup and Fable's Minor findings through implementation,
validation, and independent review. This is an execution checklist, not a replacement
for the repository's backlog tracker. Creating it does not authorize a commit or merge.

References: [implementation](2026-09-11-r8-r12-implementation.md),
[Fable review and corrections](2026-09-11-r8-r12-fable-review-summary.md),
[design](../superpowers/specs/2026-09-11-r8-r12-save-quit-safety-design.md),
[implementation plan](../superpowers/plans/2026-09-11-r8-r12-save-quit-safety-plan.md).

## Starting point

- [x] User selected Save and Quit saving all unsaved documents.
- [x] R8/R12 implementation and 16 regression tests added on the fix branch.
- [x] Initial implementation passed workspace tests (2,481 passed; 6 ignored),
  clippy, build/test compilation, and `smoke: 9/9 PASS`.
- [x] Fable reviewed that snapshot: no Critical/Important findings; conditional GO
  with seven numbered Minor findings.
- [x] Corrected Fable's test-isolation note: library unit tests already use a
  per-process temporary state directory.
- [x] Complete the follow-ups below and obtain fresh final validation/review.

The checked validation items describe the earlier snapshot. They are not evidence
that later edits pass. The earlier independent design/plan gates were not performed;
the final Fable review does not retroactively establish them.

## How to use this checklist

For each effort, check implementation only after its targeted tests pass. Record
the changed files, test command/result, and review evidence in the completion log
at the end. Update the design/plan when an agreed decision changes their requirements.
Keep policy decisions separate from mechanical cleanup. Do not delete safety coverage
merely because an old implementation path disappears.

Recommended order: 1 → 2 → 3 → 4 → 5, then the separately decided effort 6.
Effort 7 records behavior intentionally retained. Finish with the combined gates.

## 1. Remove the unreachable legacy quit chain — Fable Finding 3

- [x] Record the removal decision and amend the design/plan's explicit instruction
  to retain `PostSaveAction::Quit` before deleting it.
- [x] Reconfirm all production callers; distinguish the public `save_and_quit`
  command from the obsolete internal action and prompt.
- [x] Remove the unreachable action, completion/timeout branches, legacy prompt,
  and prompt actions that have no remaining caller.
- [x] Move applicable failure, timeout, and dirty-buffer assertions onto the live
  session-wide quit flow; remove only genuinely obsolete tests.
- [x] Verify command IDs, bindings, menu/palette access, unnamed Save As, and
  Save All behavior remain intact.
- [x] Run targeted quit/close/timeout tests and record a review of the cleanup diff.

## 2. Clarify save-request state ownership — Fable Finding 6

- [x] Separate immutable save-request identity from completion state owned by the
  waiting action; document which state is authoritative.
- [x] Audit production and synthetic-test constructors of pending actions.
- [x] Preserve exact-request matching when two saves share a buffer and version.
- [x] Verify same-path stale saves, sequential Save As, and wrong-path completion
  still have the intended behavior.
- [x] Run targeted request/completion regressions and record the result.

## 3. Match request identity on the panic path — Fable Finding 1

- [x] Add a failing regression: a different same-version save panics while another
  request is awaited; it must not cancel the awaited request.
- [x] Carry sufficient identity through the actual worker's panic outcome, not
  only through successful save merges or the test executor.
- [x] Cancel a quit/close when its own awaited save panics, preserving dirty state
  and the visible error. Keep non-save panic handling intact.
- [x] Check both InlineExecutor and ThreadExecutor behavior, including worker
  survival and completion of the next queued job.
- [x] Run targeted panic/request tests and record the result.

## 4. Consolidate destination-picker cancellation — Fable Finding 2

- [x] Route `file_browser::cancel_destination` through shared quit cleanup where applicable.
- [x] Do the same for destination redirect and empty-destination paths in
  `file_browser_commit`.
- [x] Preserve their existing picker/export/close-specific cleanup and status behavior.
- [x] Test Esc, empty submission, and export redirection while a quit save is pending.
- [x] Verify a late result cannot resurrect the cancelled quit, leave a busy latch,
  or inadvertently cancel an unrelated close action.
- [x] Run targeted cancellation tests and record the result.

## 5. Document state and resolve small consistency questions — Fable Finding 7

- [x] Document reviewed/discarded version fields and save-dispatch outcomes.
- [x] Explain why failed close-save actions use shared quit cleanup and what
  prevents incompatible pending flows from coexisting.
- [x] Explicitly decide whether Quit and Save and Quit should treat a pending close
  identically; preserve the existing safe behavior until that decision is made.
- [x] Test any resulting behavior change; otherwise document the intentional distinction.
- [x] Preserve the correction to the review's inaccurate personal-state-directory claim.
- [x] Review documentation against final code and remove stale legacy descriptions.

## 6. Define Save As behavior during quit — Fable Finding 4

**Policy approved and implemented.** Reject new user-initiated Save As operations
after a quit flow starts, with a clear message. The quit flow’s own filename requests
remain available. Already-dispatched saves finish and merge before exit.

- [x] Confirm the policy before implementing it; update the design/plan.
- [x] Distinguish manual Save As from quit-owned filename requests at all relevant
  registry, picker, and commit entry points.
- [x] Handle a manual picker opened before quit: checking only the command that
  opens the picker must not leave a later-commit loophole.
- [x] Specify how already queued Save As writes and their session/rekey completions
  are handled; do not silently drop an authorized write or necessary metadata merge.
- [x] Add FIFO interleaving tests: Save and Quit → manual Save As; existing picker
  → quit → commit; unnamed document's quit-owned Save As → success/cancel.
- [x] Verify saved bytes, destination metadata, session migration, and final quit state.
- [x] Run targeted tests and independently review this behavior change.

## 7. Retain the conservative destination-mismatch warning — Fable Finding 5

Planned disposition: keep the approved safe cancellation behavior for now.

- [x] Record that a second quit request may be needed after overlapping Save As
  and Save and Quit, even when the new path is already clean.
- [x] Keep a regression verifying the warning, cancelled pending action, and
  preserved saved-state metadata.
- [x] Verify effort 6 does not accidentally weaken this protection for saves that
  were queued before quit began.

## Combined validation and independent review

- [x] Audit the final diff: only agreed efforts changed; preserve unrelated untracked work.
- [x] Run all focused durability regressions, including promoted Fable probes.
- [x] Run `cargo test --workspace`; record exact pass/fail/ignore counts.
- [x] Run `cargo build --workspace` and `cargo test --workspace --no-run`, warning-free.
- [x] Run `cargo clippy --workspace --all-targets`, warning-free. Do not run rustfmt.
- [x] Run the isolated PTY smoke suite and quote its exact advisory summary.
- [x] Refresh the source patch, hashes, implementation report, and validation links.
- [x] Ask Fable to review the complete revised snapshot, with clippy/version/hash
  commands available; retain its report and probes.
- [x] Resolve Critical/Important findings and rerun review after fixes. Give every
  remaining Minor finding an explicit disposition supported by evidence.
- [x] Complete the separate Codex pre-merge check required by the project workflow;
  record the verdict and distinguish it from implementation self-review.
- [x] User authorized local commit and merge on 2026-09-11; push was not requested.
- [x] Applied the authorized merge to local main without conflicts and verified the merged tree: 2,499 passed, 0 failed, 6 ignored.

## Completion log

Add a row when an effort advances. An unchecked item remains outstanding unless a
dated, explicit decision explains its deferral or replacement.

| Effort | Changed files / decision | Validation evidence | Review / disposition |
| --- | --- | --- | --- |
| Initial R8/R12 implementation | See linked implementation report | Initial gates passed | Fable conditional GO; follow-ups tracked below |
| 1 — Legacy chain | Removed unreachable action/prompt/timeout chain; ported live-flow and mouse/render coverage | Workspace: 2,499 passed, 6 ignored | Codex GO; final Fable GO |
| 2 — Request state | Immutable request identity; completion flag belongs to pending action | 33 focused durability tests passed | Codex GO; final Fable GO |
| 3 — Panic identity | Shared inline/threaded execution boundary transports exact request | Foreign/own panic plus worker-survival regression passed in both executors | Codex GO; final Fable GO |
| 4 — Cancellation | All three destination paths use shared cleanup; unrelated close preserved | Esc, empty, export redirect, late completion and close-preservation tests passed | Codex GO; final Fable GO |
| 5 — Docs/consistency | State docs updated; existing pending-close distinction retained; isolation correction preserved | Code/docs review and clippy passed | No behavior unification requested |
| 6 — Save As policy | User approved; opening/submission/overwrite guarded; existing saves merge before exit | Ownership, queued migration, failure/timeout, and real-plugin regressions passed | Codex GO; final Fable GO |
| 7 — Warning | Conservative destination-mismatch cancellation retained | Save As-before-quit regression passed | Intentional behavior |
| Runtime exit correction | Independent review found post-pump exit gap; shared final barrier added | Real-plugin failing reproduction, then 2 passing end-to-end regressions; final discard/cancel tests passed | Initial Codex NO-GO resolved by round two GO |

| Fable cleanup nits | Registry picker closure, shared cancellation, status wording, comment/guard clarity | Maintained replacement/status regressions; workspace gates | Full cleanup Minor items fixed or explicitly deferred in implementation report |
| Mouse cancellation I1 | Click-away uses quit-owned picker cleanup; non-quit behavior preserved | Real input-path regression red then green | Codex round four GO; final Fable GO, I1 resolved |

Current evidence: [cleanup implementation report](2026-09-11-premerge-cleanup-implementation.md),
[final Codex review](evidence/2026-09-11-premerge-cleanup/codex-review-round4.md),
[final Fable verification](evidence/2026-09-11-premerge-cleanup/fable-review.md),
[validation logs and patch](evidence/2026-09-11-premerge-cleanup/).
Final validation: 33 focused durability tests plus 2 real-plugin regressions;
workspace 2,499 passed, 0 failed, 6 ignored; warning-free clippy/build/test compilation;
`smoke: 9/9 PASS`. Both independent review chains end in GO. Remaining Minor findings
have explicit dispositions in the cleanup report (including final Fable N1/N2).
Implementation, validation, and review are complete. User authorized local commit and
merge on 2026-09-11. The merged tree passed all 2,499 workspace tests (6 ignored);
all 26 source hashes match the reviewed snapshot. Implementation commit: `fbb2a1c`.
Merged-tree evidence: [workspace test log](evidence/2026-09-11-premerge-cleanup/merged-workspace-tests.log).
Push is not requested.
