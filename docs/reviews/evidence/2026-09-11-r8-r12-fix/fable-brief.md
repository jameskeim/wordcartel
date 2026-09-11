# Independent Fable implementation review

Review the complete uncommitted R8/R12 implementation on `fix/quit-save-durability`,
against base `d3aea924988f60702529f6c68a0e332e64d44f1d`.

The user approved Save and Quit saving all unsaved documents. This is the final
whole-implementation review described in CLAUDE.md. Do the review, not another
planning or implementation cycle. Give independent verdicts on design compliance
and code quality. Classify your own findings as Critical / Important / Minor.

Read these artifacts, then verify their claims against actual source and callers:

- `docs/superpowers/specs/2026-09-11-r8-r12-save-quit-safety-design.md`
- `docs/superpowers/plans/2026-09-11-r8-r12-save-quit-safety-plan.md`
- `docs/reviews/2026-09-11-r8-r12-implementation.md`
- `docs/reviews/evidence/2026-09-11-r8-r12-fix/source-sha256.json`
- `git diff` (includes intent-to-add entries for the two new Rust files).

The changed source includes save completion/request identity, shared quit flow,
version-specific discard decisions, and regression tests. Review interactions with
callers, pending actions, buffer replacement/close, Save As, plugin callbacks, error
and cancellation paths, and production event-loop ordering. These examples are
starting points, not a restriction on what to flag. Distinguish new defects from
relevant pre-existing behavior; report unresolved safety/design issues clearly.

This checkout is an isolated copy with the exact reviewed working-tree contents.
Do not access or modify the original checkout. You may add temporary reproduction
tests and compile/run them HERE. Preserve any useful probe source under
`review-probes/`. Do not implement fixes, commit, push, merge, or contact external
people/services. No web research is needed for this source review. Do not inspect
credentials or unrelated files. Do not spawn additional reviewers.

Existing workspace tests, clippy, build/test-build and smoke passed; logs are in
`docs/reviews/evidence/2026-09-11-r8-r12-fix/`. Treat that as scoped evidence, not a
reason to trust the implementation. Use focused tests when they can resolve a
hypothesis. Never run rustfmt. Use temporary documents and the existing Fs seam;
XDG state/config/cache and the build target are isolated by the launcher.

Write `FABLE_REVIEW.md` in this checkout with:

1. Separate design-compliance and code-quality verdicts, and a merge recommendation.
2. Findings ordered by impact: affected file/symbol/line, trigger, expected versus
   actual behavior, impact, and reproduction or source evidence.
3. Commands/checks run and results, preserved probe paths, and practical coverage limits.
4. If no findings, say so explicitly and state what was checked.

Your final response should summarize the verdict and findings. Do not edit the
implementation to make a test pass. If a needed tool is denied, report that limit
and continue source review where possible.
