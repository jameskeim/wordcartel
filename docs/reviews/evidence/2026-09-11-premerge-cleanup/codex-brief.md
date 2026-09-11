# Independent Codex pre-merge check

Review the current uncommitted source on `fix/quit-save-durability`, against
`d3aea924988f60702529f6c68a0e332e64d44f1d`. Include untracked `wordcartel/src/quit.rs`
and `wordcartel/src/durability_regressions.rs`; git diff alone omits them.

The user approved Save and Quit saving every unsaved ordinary document. They also
approved removing the unused legacy quit chain, addressing the Fable Minor findings,
and blocking new manual Save As operations during quit while preserving quit-owned
filename prompts and letting previously dispatched saves finish and merge.

Read the current design and checklist, then inspect real code and callers:
- docs/superpowers/specs/2026-09-11-r8-r12-save-quit-safety-design.md
- docs/reviews/2026-09-11-r8-r12-premerge-checklist.md
- docs/reviews/2026-09-11-r8-r12-fable-review-summary.md (historical review of the prior snapshot)

This is an independent static code review, not implementation or another planning
cycle. Do not edit source, run cargo, or delegate. The parent is running validations
concurrently. You may write only your report under this evidence directory.

Check identity/version correctness, post-save completion and panic routing, final
quit conditions, discard decisions, cancellations, reentry, closed buffers, manual
versus quit-owned Save As (including existing pickers and overwrite prompts), in-flight
save accounting, and the timing of metadata merges relative to exit. These are starting
points, not a restriction on findings. Distinguish new issues from pre-existing ones.

Write codex-review.md in this directory. Give independent design-compliance and
code-quality verdicts plus GO/NO-GO. Report Critical/Important/Minor findings with
source locations, reachable triggers, expected/actual behavior and evidence. Do not
assume a passing test or the earlier Fable review establishes correctness. State scope
and limits if clean. Final response should summarize verdict and report path.
