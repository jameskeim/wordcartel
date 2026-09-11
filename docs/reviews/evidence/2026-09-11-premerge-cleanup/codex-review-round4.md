# Independent Codex focused review — round 4

Reviewed 2026-09-11: the mouse click-away follow-up on `fix/quit-save-durability`. Earlier reports are preserved.

- **Design compliance: PASS for the requested minimal fix.**
- **Code quality: PASS.**
- **Recommendation: GO for this delta**, subject to final validation and explicit merge authorization.
- **New findings: Critical 0; Important 0; Minor 0.** Fable I1 is resolved. The expressly deferred existing findings remain deferred, not fixed.

`mouse_file_browser` now routes a left click outside the picker through `file_browser::close_overlay` (`mouse.rs:536`). I traced the helper: when `quit_save_owner` is present, it calls `cancel_destination`, which removes the picker and pending filename/overwrite state and calls `quit::cancel`. That clears the quit drain, advancement latch, and provisional quit flag. The click therefore cannot leave the orphaned ownership reported in I1.

The helper's other branch still performs only `editor.file_browser = None`. Manual Save As pickers, ordinary file browsers, and close-buffer-owned filename pickers have no quit owner, so their click-away semantics are unchanged. This implements Fable's option (a), without silently widening to cancellation of every destination flow. Successful quit-owned commits still remove the picker before opening overwrite confirmation; this mouse-only change does not affect that transition or the post-callback exit barrier.

The maintained regression `mouse_click_away_cancels_a_quit_owned_filename_request` (`durability_regressions.rs:749`) feeds a real mouse event through `app::reduce`, then checks absent picker, pending filename action, and drain, verifies no quit, and successfully opens manual Save As afterward. I read `mouse-red.log`, which fails at the stale-ownership assertion, and `mouse-green.log`, which records this test passing. These are controller-run logs; I did not execute cargo.

I acknowledge the explicit deferrals: plugin-only replacement of a quit-owned overwrite prompt can still orphan that request, and pre-existing click-away behavior for a close-buffer Save As picker can retain its close action. Neither is changed by this delta; this report does not claim they are resolved. Fable's findings and their deferrals should remain visible in the final evidence/issue ledger.

This was a focused static review of the changed mouse route, shared closer/cancellation helpers, regression, and supplied red/green evidence. Only this report was written; no source edits, tests, or delegation were performed. GO does not authorize commit, push, or merge.
