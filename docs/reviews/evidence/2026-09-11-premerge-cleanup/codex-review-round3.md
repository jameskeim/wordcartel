# Independent Codex pre-merge review — round 3

Reviewed 2026-09-11: current uncommitted `fix/quit-save-durability` source after the Fable follow-ups. Prior Codex reports are preserved.

- **Design compliance: PASS.**
- **Code quality: PASS.**
- **Recommendation: GO from this static review**, subject to final validation gates and explicit merge authorization.
- **New findings: Critical 0; Important 0; Minor 0.**

## Changes and interactions checked

**Picker replacement:** The file-browser overlay row now calls `file_browser::close_overlay` (`overlays.rs:142`). That helper delegates to full destination cancellation only when the discarded browser carries quit ownership (`file_browser.rs:402-407`). Consequently replacing a quit-owned picker with a palette, prompt, or another overlay clears its pending filename action, drain, advancement latch, and provisional quit flag. Ordinary browsers retain their previous simple close behavior. The helper neither recurses through the overlay registry nor cancels unrelated awaited close saves.

I traced the successful and cancelled transitions as well as the new regression. Successful destination commit removes the browser before opening overwrite confirmation (`file_browser_commit.rs:461-472`), so the new overlay closer cannot abort the valid quit-owned overwrite step. Export redirection explicitly cancels and clears the old ownership before opening its new picker. Empty submission also clears ownership, allowing a later manual retry. A new summary replacing a quit picker can still start a fresh flow; direct Save and Quit reentry remains refused while busy. The regression `replacing_quit_owned_picker_with_palette_cancels_its_ownership` exercises registry-based overlay closure and verifies that manual Save As becomes available afterward.

**Shared cancellation:** The awaited panic, unsuccessful synthetic completion, and quit-save timeout branches now use `quit::cancel`. They clear their own pending save action before cancellation where appropriate; the shared helper also revokes provisional exit. Exact-request matching on the panic path remains intact, so a different same-version save's panic cannot cancel an explicitly awaited request. Writes remain tracked until foreground completion and are not cancelled by abandoning the quit flow.

**Cancellation status:** `save_finished` now returns whether a tracked failed/mismatched completion actually cancelled a drained wait (`quit.rs:155-167`). The normal merge and panic caller append the cancellation explanation to their original warning/error. The return value does not change the cancellation predicate or request accounting. An explicitly awaited different request remains protected. The changed-destination regression checks both the underlying warning and the quit-cancelled wording while preserving the successfully rekeyed document path.

**Final exit:** I rechecked the real runtime's post-pump `finish_iteration` barrier, retained discard decisions, `after_callbacks` rescan, and manual Save As guard through provisional exit. The cleanup does not reopen the round-one plugin timing defect. A callback edit is still resaved/reviewed, a callback-dispatched save still prevents exit until its merge, and cancellation revokes provisional quit. Existing session migrations still drain before actual exit.

The guarded `expect` replacements and clarified legacy comment do not alter behavior.

## Scope and limits

This was an independent static review of the current follow-up code and its surrounding quit/save, overlay, prompt, worker-result, timeout, and runtime callers. I also read Fable's report and the two new regression bodies. I did not treat either prior GO or test assertions as proof, did not run cargo, did not edit source, and did not delegate. Only this report was written.

The controller reports 32 passing durability regressions; final combined validation remains separate. Fable's pre-existing extra conflict prompt and quit-summary concurrency observations are not newly introduced by these follow-ups. Documentation/checklist refresh and final evidence assembly remain the controller's responsibility. This review does not expand the approved scope to external-writer races, symlink retargeting, or recovery identity/cadence, and does not authorize commit, push, or merge.
