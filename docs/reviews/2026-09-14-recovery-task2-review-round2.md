# Recovery Task 2 independent review — round 2

Date: 2026-09-14. Follow-up to
[round 1](2026-09-14-recovery-task2-review.md), against design revision 5 and plan
revision 2 Task 2. Static review of the added integration tests; production source
is unchanged from the preceding reviewed snapshot. No cargo commands or source
edits were performed by this reviewer.

**Spec/plan compliance: GO. Code quality: GO.**
Open findings: 0 Critical / 0 Important / 0 Minor.

I1 is resolved. `recovery_ownership_fresh_slot_retries_every_failed_ancestor_barrier`
injects every first-checkpoint directory-sync failure, checks the selected occurrence
was reached, drops the original slot, and retries through a fresh slot over retained
directories. `assert_full_sync_chain` compares exact journal paths, in order, against
every resolved ancestor from owner directory through filesystem root. This is a
durability-operation assertion, not merely a successful return assertion.

`recovery_ownership_partial_mkdir_retries_keep_full_sync_obligation` covers failure at
state-root, protocol-directory and owner-directory creation. It retries both with the
same slot and with a fresh slot, retaining partially provisioned ancestors and requiring
the same complete sync chain before acknowledgement. Together with the unchanged
collision/tombstone and same-path independent-slot tests, these close the missing
reconstruction/provisioning coverage identified in round 1.

The inspected `task2-integration.log` reports all 5 integration tests passing. The
previous 11-test store evidence and round 1 production analysis still apply. No new
correctness issue was found in the added tests. This closes Task 2 only; later
discovery, save cleanup, handoff and UI tasks still require their own implementation
and review. No Windows runtime or physical power-loss guarantee is inferred.
