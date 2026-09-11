# Independent Codex pre-merge review

Reviewed 2026-09-11: current uncommitted source on `fix/quit-save-durability`, against `d3aea924988f60702529f6c68a0e332e64d44f1d`, including untracked `wordcartel/src/quit.rs` and `wordcartel/src/durability_regressions.rs`.

- **Design compliance: FAIL at the final runtime exit boundary.** The save/quit state-machine implementation otherwise follows the approved policy.
- **Code quality: changes required.** Request identity, destination-aware merges, and version-specific discard state are clear, but the runtime integration does not uphold their final invariants.
- **Recommendation: NO-GO.** Critical: 0; Important: 1; Minor: 0.

## Important 1 — Plugin command execution occurs after the final dirty/in-flight check

**Locations:** `wordcartel/src/jobs_apply.rs:188-195`, `wordcartel/src/app.rs:841-846` and `:915`, `wordcartel/src/quit.rs:99-107`. Supporting callers: `wordcartel/src/registry.rs:953-974`, `wordcartel/src/plugin/pump.rs:109-138`, `wordcartel/src/plugin/api.rs:331-350` and `:488-517`.

**Reachable trigger:** Start Save and Quit on a dirty named document. While its last save is running, invoke a registered plugin editing command through its keybinding or palette. The registry queues the plugin callback; it does not execute it during dispatch. Have the last save result become ready for the same reduce call's executor drain. No synthetic state or direct internal API invocation is necessary: user input dispatch and asynchronous save completion can naturally share that iteration.

**Expected:** Any accepted command that edits a document before actual exit must participate in the final live-version decision. New manual Save As must remain blocked through exit, and any dispatched save must receive its foreground merge before exit.

**Actual:** The reduce epilogue applies the last save and `drive_quit_drain` sees no dirty buffers or outstanding saves. It clears `quit_drain` and sets `editor.quit = true`. `reduce` returns false, captured as `keep` in `app.rs:841`. The runtime then unconditionally calls `plugin_host.pump` at line 846. That pump executes the already queued normal plugin callback. A callback using `wc.insert` changes the document now, after the final scan. The runtime still breaks on the old `keep` at line 915. The new content was neither saved nor discarded by a decision for its version.

The same ordering exposes the approved Save As/accounting policy: a normal callback calling `wc.command('save_as')` dispatches during the pump after the drain was removed. `quit::in_progress` does not include `editor.quit`, so the manual picker opens and is immediately abandoned at exit. A callback calling `wc.command('save')` instead starts a new tracked write after the final in-flight check; the process joins the worker on shutdown, but never processes that new result's foreground merge. The observer restriction does not prevent these triggers: it blocks editing/command dispatch from event hooks and timers, while normal plugin command callbacks explicitly retain those capabilities.

**Origin and scope:** The pump-after-reduce ordering predates this branch. This is an unresolved integration defect in the new claim that the live-version/in-flight scan is final, and a loophole in the newly approved manual Save As restriction. The local state-machine tests stop after job application and therefore cannot establish the runtime guarantee.

**Required correction:** Make the actual exit decision occur after all permitted foreground mutation/dispatch stages, preserving quit-mode and version-specific discard decisions until that point, or prevent accepted queued mutating work from executing after the terminal decision with an explicit safe disposition. Keep manual Save As blocked through the terminal phase. Add a regression that queues a real plugin command during the reduce that consumes the last save result, runs the real pump, and verifies both edited-content handling and in-flight completion handling. Merely adding `editor.quit` to the Save As guard does not fix the editing/ordinary-save variants.

## Evidence for the portions that comply

- Both production save constructors bind an immutable unique `SaveRequest`; awaited completion lives in `PendingAfterSave.completed`. `Job::execute` preserves request identity across panics for both executors. Success and panic handling distinguish same-buffer/same-version requests.
- Ordinary save merges compare the captured chosen destination with the live document path before changing saved-version/fingerprint/checkpoint state. Mismatches still report the actual write and produce the conservative warning/cancellation. Same-path older snapshots and sequential Save As rekeys remain supported.
- Quit refills from live ordinary dirty buffers and excludes only explicitly discarded `(BufferId, version)` pairs. Review Save targets the reviewed ID/version; stale reviews are shown again. Buffer allocation uses monotonic IDs, and closed IDs cannot authorize replacement buffers.
- Conflict/rejected dispatch does not masquerade as an awaited save. Picker escape, empty submission, export redirection, explicit cancellation, save failure, and timeout clear the applicable quit state without cancelling already dispatched writes. Dirty close operations refuse overlapping flows; ordinary Quit intentionally supersedes a pending close, while Save and Quit refuses it.
- Picker opening, submission, and the Save As write boundary enforce manual versus quit-owned requests before the terminal gap above. Quit-owned overwrite confirmation rejects a changed active buffer.
- The in-flight set is removed on foreground save completion/panic, including durability results for closed buffers. Drained quit waits for existing saves and has a timeout. Ordinary Quit waiting on clean-workspace saves selects Review Each if an edit appears before the final drain.
- Existing queued Save As migration records are drained by `app.rs:905-912` before the ordinary loop break and again during shutdown. This ordering correctly persists migrations already merged; it does not merge jobs newly dispatched by the later plugin stage described above.
- The unreachable `PostSaveAction::Quit`, old quit prompt, and their prompt actions were removed. The public `save_and_quit` registry handler remains on the shared flow.

## Scope and limitations

This was an independent static review of the design, checklist, historical Fable summary, source changes, untracked implementation/tests, and relevant registry, overlay, runtime, plugin, workspace, and session callers. I read the regression assertions as evidence of intended coverage, not as proof that they pass. Per the brief, I did not run cargo, modify source, or delegate. The parent is validating separately. External-writer races, symlink retargeting, recovery identity/cadence, and broader pre-existing editor behavior were not audited comprehensively. No commit, push, or merge is authorized by this report.
