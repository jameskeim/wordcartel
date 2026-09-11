# Independent Codex pre-merge review — round 2

Reviewed 2026-09-11: revised uncommitted source on `fix/quit-save-durability`, against `d3aea924988f60702529f6c68a0e332e64d44f1d`, including untracked `quit.rs` and `durability_regressions.rs`. The first report is preserved in `codex-review.md`.

- **Design compliance: PASS.**
- **Code quality: PASS.**
- **Recommendation: GO from this static review**, subject to the separately required validation gates and explicit merge authorization.
- **Open findings: Critical 0; Important 0; Minor 0.**

## Round-one Important finding: resolved

The runtime no longer treats reduce's pre-pump result as the final exit decision. `app::run` calls `finish_iteration` after the real plugin pump and subsequent foreground setup stages (`app.rs:852-902`). `finish_iteration` delegates to `quit::after_callbacks`, which revokes the provisional flag, reruns the live dirty/in-flight decision using the retained quit mode and discard decisions, and clears the drain only after that decision succeeds (`quit.rs:173-182`). The actual loop break uses this new result. I inspected the stages after the barrier: advance/render/session persistence do not execute another plugin callback or dispatch a document save.

This closes each reported variant:

- A queued plugin edit after the last save merge is seen by the post-pump rescan. Save All resaves the new version; Review Each requires a fresh decision.
- A normal save dispatched by the callback is present in `saves_in_flight`, so exit waits for its foreground merge. An eventual failure or timeout still cancels the waiting quit.
- `in_progress` now includes `editor.quit`, so the manual Save As guard remains active during provisional exit, including ordinary Quit without an existing drain.
- `quit::cancel` revokes the provisional flag, so a cancellation cannot be overwritten by the old reduce result.
- `drive_quit_drain` preserves the drain at provisional exit. Unchanged discarded versions remain authorized at the final rescan; edits invalidate those decisions without re-prompting for unchanged documents.

Both end-to-end harness paths now call the same barrier after their real plugin pump. The new tests in `e2e.rs:535-588` exercise an actual registered Lua callback selected through the palette while the last save result is ready: one edits, and the other dispatches Save and Save As. Their assertions cover saved bytes, continued execution until the extra merge, empty final in-flight accounting, and refusal to open the late Save As picker. The final-discard and final-cancel tests in `durability_regressions.rs:674-709` cover the retained-decision and revoked-exit cases.

I read the supplied validation logs: `post-pump-red.log` records the first plugin test failing on the expected lost-edit assertion before the fix; `post-pump-tests.log` records both plugin regressions passing; `focused-tests.log` records 30 durability regressions passing. These are controller-run evidence, not tests independently executed by this reviewer.

## Broader recheck

I also rechecked the final changes against the first review's save/request/picker findings and followed their interaction with provisional exit. No additional actionable defect was found:

- Exact immutable request identity still governs successful and panicked completion, independently of equal buffer/version values; both executors share the panic boundary.
- Destination mismatch still preserves current-path saved metadata and checkpoint state, reports the completed destination, and conservatively cancels the awaited action.
- Live-version discard decisions, stale review targeting, scratch exclusion, monotonic buffer identity, and closed-buffer durability merges retain their intended behavior.
- Conflict/rejection, picker and overwrite cancellation, Save As ownership checks, reentry, close/quit exclusivity, and timeout handling remain intact. New manual Save As is blocked while the quit owns state or a provisional exit.
- Existing save merges and session migrations still precede actual exit; the new barrier can dispatch further required saves only while returning keep-running.
- The obsolete internal quit action/prompt chain remains removed, while the public `save_and_quit` command remains registered on the shared all-document flow.

## Scope and limits

This is an independent static re-review, building on the original full source/caller review and checking the revised integration broadly. I did not run cargo, edit source, or delegate. Only this report was written. Passing supplied tests did not substitute for tracing the runtime ordering. Workspace/build/clippy/smoke gates are the parent's responsibility and were not independently certified here. External-writer conflict races, symlink retargeting, and recovery identity/cadence remain outside this change's stated scope. GO is a review verdict, not authorization to commit, push, or merge.
