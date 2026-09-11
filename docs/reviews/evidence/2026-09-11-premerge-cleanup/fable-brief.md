# Fable verification of I1 — mouse click-away

Verify the fix for the single blocking I1 in previous-fable-review.md. The previous
review fully assessed the implementation and explicitly recommended GO once I1
was fixed and re-verified. All other production code is unchanged from that review.

Read followup.diff and the actual current code. The entire production delta is one
line: mouse_file_browser click-away now calls file_browser::close_overlay(editor)
instead of assigning file_browser = None. This is your minimal option (a), preserving
manual/non-quit picker behavior. The maintained test
mouse_click_away_cancels_a_quit_owned_filename_request uses real app::reduce with
mouse input. mouse-red.log reproduces I1; mouse-green.log passes after the fix.

The current snapshot is already applied, and source-sha256.json identifies it.
Check the routing, shared helper, and regression; run the focused test if useful.
Do not repeat unrelated full-branch investigation unless this one-line change
warrants it. Report any actual new issue, but do not assume earlier approval
establishes correctness of the delta.

Prior M2 (plugin-only closure of quit-owned overwrite prompt), M3 (pre-existing
close-buffer picker click-away), and prior retained fingerprint/summary UX issues
are explicitly deferred; they are not claimed fixed. No choice to broaden manual
picker cancellation was made. No production changes other than I1 are in this delta.

Use this isolated checkout only. Temporary probes may be added here and preserved
under review-probes; restore source afterwards. Do not implement fixes, run rustfmt,
commit, push, merge, delegate, inspect credentials or contact other services.

Write FABLE_REVIEW.md: I1 resolved/unresolved, independent design/code-quality
verdicts and GO/NO-GO for the current snapshot, tests/evidence used, any new findings,
and retained limitations. A concise focused report is sufficient.
