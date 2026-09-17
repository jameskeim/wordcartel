# Independent recovery polish re-review

Review the revised full recovery-safety snapshot, concentrating on the two user-authorized
refinements since your prior round-2 GO: acknowledgement after opening-batch cancellation
and clarity of metadata-less busy rows. Read:
- docs/reviews/2026-09-15-recovery-polish-design-plan.md
- docs/reviews/2026-09-15-recovery-polish-design-review.md
- docs/reviews/2026-09-15-recovery-polish-progress.md

Prior approved report, full source diff, source manifest and validation are in
 docs/reviews/evidence/2026-09-15-recovery-polish/previous-final-review/.
The new source.patch/manifest cover the complete branch. polish-source.patch identifies
changes from that prior reviewed implementation. Check those changes against actual source
and cross-module invariants; do not infer correctness merely from earlier GO or green tests.

The user retained separate follow-ups for general recovery-file reclamation and inactive
origin auto-offers. Busy wording must not invent ownership/empty-record metadata, perform
foreground IO, or make unavailable rows selectable. Cancelled completion may reflect a
valid durable acknowledgement, but may not permit source retirement, install cancelled
preparations, mark newer edits protected, mutate a replacement slot, or affect a later quit.

Use this isolated checkout for any focused compiled probes. Save probes/output under
review-probes/. No live-tree edits, commits, merges, pushes, messages to humans, or fmt.
CLAUDE.md describes the existing final reviewer role; do not restart the whole implementation
pipeline or spawn agents. Write FABLE_REVIEW.md with GO/NO-GO, severity findings, concrete
triggers/evidence/fixes, commands/results and limitations. Restore any temporary source
probe wiring and recheck the source manifest before finishing.

Linux validation does not establish Windows compilation/runtime durability. Process death
is not a power-loss simulation. Quit timeout does not bound OS IO or worker joining.
