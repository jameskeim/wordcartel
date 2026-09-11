# Independent Fable review — R8 / R12

Reviewer: Claude Fable 5.1 (`claude-fable-5-1`, resolved from the requested `fable` alias).
Reviewed snapshot: `fix/quit-save-durability`, uncommitted implementation against
`d3aea924988f60702529f6c68a0e332e64d44f1d`.

## Fable's verdict

- **Design compliance: PASS.**
- **Code quality: PASS with Minor findings.**
- **Critical: 0. Important: 0.**
- **Merge recommendation: conditional GO**, pending an explicit decision about
  retaining the legacy quit path or removing it in a follow-up.

The [full report, verbatim](evidence/2026-09-11-r8-r12-fix/fable-review.md) contains
seven numbered Minor findings. This summary records the external review; it is not
a claim that those follow-ups have been fixed or that a merge was authorized.

## Minor findings ledger

| # | Observation | Disposition |
| --- | --- | --- |
| 1 | Save panic handling still matches buffer/version, not request ID, so a different same-version save's panic can cancel a waiting quit | Safe cancellation, no data loss; follow-up |
| 2 | Three destination-picker cancellation sites duplicate cleanup rather than calling `quit::cancel` | Pre-existing inconsistency; consolidate in follow-up |
| 3 | Legacy `PostSaveAction::Quit` and its prompt/timeout chain are no longer reached by production dispatch | Retained by the design; explicit retain/remove decision pending |
| 4 | A later queued Save As can write after quit is set, while its rekey/session merge is never applied | Pre-existing bookkeeping issue; follow-up with Save As concurrency policy |
| 5 | Save As followed by Save and Quit can safely cancel quit with a warning even when the new path is already clean | Spec-consistent conservative behavior; no implementation change proposed here |
| 6 | `SaveRequest` is a Copy value carrying a mutable completion flag | Correct as implemented; consider clearer separation of request identity and pending-action state |
| 7 | Documentation/style details and differing pending-close behavior between quit entry points | Follow-up; one factual correction below |

Recommendation: handle the unreachable legacy path as an explicit cleanup decision,
and keep the small cancellation/documentation fixes separately reviewable. Do not
treat these notes as authorization to change the approved behavior or merge.

## Validation performed by Fable

- Verified all eleven implementation source hashes in the supplied manifest.
- Read the full working-tree diff and related callers/state transitions.
- Independently ran the 16 new durability regression tests: all passed.
- Wrote and ran 15 additional probes: all passed, with observed states supporting
  the Minor findings. [Preserved probe source](evidence/2026-09-11-r8-r12-fix/fable-probes/review_probes_fable.rs).
- Ran `cargo test -p wordcartel`: 2,064 passed, 0 failed, 6 ignored across ten
  summaries, as recorded in its report. These overlap the 16 regressions; the counts
  should not be added together as distinct coverage.
- Did not independently rerun clippy, smoke, or a separate build/test-build command.
  The earlier implementation checks remain recorded in the
  [implementation report](2026-09-11-r8-r12-implementation.md).

The review ran in an isolated copy under `/tmp`, with its own copied local `repar`
dependency and isolated state/config/cache/build directories. The original working
tree's eleven source hashes were rechecked after Fable finished and remain unchanged.
The launcher retained Fable's [event log](evidence/2026-09-11-r8-r12-fix/fable-events.jsonl),
[result metadata](evidence/2026-09-11-r8-r12-fix/fable-result.json), and
[invocation manifest](evidence/2026-09-11-r8-r12-fix/fable-run.json).

## Controller verification notes

1. **Correction to Finding 7's test-isolation statement:** the regression is a
   library unit test compiled with `cfg(test)`. `swap::state_dir` uses
   `temp_dir()/wcartel-test-state-<pid>/wordcartel` in that build, independently of
   XDG settings. It does **not** write the personal state directory. The relevant
   source is [`swap::state_dir`](../../wordcartel/src/swap.rs).
2. The clippy rerun was denied by the review launcher's restricted command allowlist,
   not by a clippy failure. The launcher has been updated to allow clippy, version,
   and hash commands in future runs; this did not change the completed review's
   permissions or retroactively execute those commands.
3. The comment-only `review_probes_fable.rs` stub mentioned in the report exists
   only in the isolated checkout. It was never created in the user's fix branch.
   Fable restored its copied `lib.rs` to the supplied hash before finishing.
4. Fable's report is preserved verbatim. These factual notes accompany it rather
   than silently rewriting the independent review.

No production edits, commits, pushes, or merges were performed during this review.
