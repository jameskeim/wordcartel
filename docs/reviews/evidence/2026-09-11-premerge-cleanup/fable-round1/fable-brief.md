# Independent Fable re-review — final pre-merge cleanup

Review the complete current implementation of R8/R12 and the approved follow-up
checklist on `fix/quit-save-durability`, uncommitted against
`d3aea924988f60702529f6c68a0e332e64d44f1d`. This is a new snapshot, not the earlier
eleven-file implementation. Verify `source-sha256.json` in this evidence directory.

Read the current design and implementation, then verify actual source and callers:

- docs/superpowers/specs/2026-09-11-r8-r12-save-quit-safety-design.md
- docs/reviews/2026-09-11-premerge-cleanup-implementation.md
- docs/reviews/2026-09-11-r8-r12-premerge-checklist.md
- git diff HEAD (new quit.rs/durability_regressions.rs have intent-to-add entries)

The user approved Save and Quit saving all ordinary unsaved documents; removing
the dead legacy quit chain; immutable save IDs with completion on pending actions;
request identity on panic outcomes; shared cancellation; and blocking new manual
Save As during quit while allowing quit-owned prompts and merging already-dispatched
saves before exit. Existing conservative destination-mismatch cancellation stays.
Ordinary Quit still supersedes a pending close; Save and Quit preserves its safe busy refusal.

The updated implementation also has a final post-plugin-callback exit barrier. A
pre-pump quit decision is provisional; retained discard decisions and live dirty /
in-flight state are rechecked after callbacks. Check its placement in the real run
loop and harness, not just its helper. Review new issues broadly, including callers,
job completion/panics, cancellation/reentry, picker and overwrite origins, failures,
timeouts, and save/session metadata. These are starting points, not restrictions.

The 30 focused durability tests, 2 real-plugin exit regressions, full workspace suite
(2,496 passed; 6 ignored), clippy, build/test-build, and smoke (9/9) pass. Logs are in
this evidence directory. Independently run appropriate checks and use small compiled
probes to resolve concrete hypotheses; don't treat supplied green logs as proof.

This is an isolated copy, with a copied local repar dependency and isolated build /
state/config/cache directories. Do not access or modify the original checkout.
Temporary reproduction tests are allowed HERE; keep useful probe sources under
review-probes/. Do not implement fixes, run rustfmt, commit, push, merge, delegate,
contact other services, inspect credentials, or read unrelated files. Restoring your
own temporary probe registration and deleting your own scratch files within this
copy is permitted; Python file operations are available for that housekeeping.

Write FABLE_REVIEW.md in this checkout. Give separate design-compliance and
code-quality verdicts plus GO/NO-GO; classify your own findings Critical / Important /
Minor, with file/symbol/line, reachable trigger, expected/actual behavior, impact,
and source or reproduction evidence. Distinguish pre-existing limitations from new
defects. List checks actually run, probe paths, and practical coverage limits. If
clean, state that explicitly. Return a concise final summary.
