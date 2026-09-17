# Final independent Codex recovery review — round 2

Date: 2026-09-15. Baseline: `9e409c0f09d9683c40e84dbd2499c9f9e6e32ded`.

**Code-review verdict: GO.** Remaining findings: **0 Critical / 0 Important / 0 Minor**.
This supersedes the earlier Codex verdict for readiness of the revised source; the
2026-09-14 report and original Fable report remain preserved as review history.
It does not substitute for the separately required final Fable review or authorize
commit, merge, or push.

## Snapshot and method

Reviewed the live recovery-safety source and its changes from the prior independently
reviewed snapshot, against the approved design/plan and their 2026-09-15 amendments.
Read the original Fable C1, I1–I6 and M1–M6 findings, follow-up triage, production-fix
notes, retry review, and associated regression coverage. Rechecked the revised
cross-module ownership, receipt, timeout, scheduling and selector transitions rather
than treating the scoped reviews or passing tests as proof.

Independently verified all **45 source-file hashes** in both the live workspace and
`/tmp/wordcartel-fable-r_pbnxbw/checkout` against
`evidence/2026-09-14-recovery-implementation/source-sha256.json`.
The verified manifest SHA-256 is:

`75c95eb3545b59b88e0fb71309a8e0280a415a80bfd8a00cca1edd89fbb8f632`

Also verified the manifest and all five final gate-log hashes against `validation.json`.
This reviewer performed read-only inspection and hash checks, with no cargo invocation
or production-source modification.

## Revised behavior and finding dispositions

| Original item | Independent conclusion |
| --- | --- |
| C1 test-module inception | Resolved by explicit parent module names/paths while retaining source-guard-compatible inner test modules. Final all-target clippy evidence is clean. |
| I1 background timeout and lost Ack | Resolved. `pending_deadline` and `timeout_tick` require an active quit and concrete handoff/association work. Ordinary background checkpoints and preparations are excluded. Timeout detaches rather than explicitly cancelling checkpoint metadata; matching late success can latch. Detached late failure cannot cancel a later quit. Handoff timeout cancels only pending retirement authorization, keeping its worker guards and safe completion routing. |
| I2 first Save As retention | Resolved under the explicit user-approved amendment. Save captures its previous path with its slot/version/content. `AssociationPolicy::SaveAs` accepts the committed, pathless, or captured previous association; ordinary Save retains the narrower policy. No source/predecessor/foreign-owner deletion right is introduced. |
| I3 repeated unavailable offers | Resolved. Unavailable rows have dismissal identities independent of selectable tokens. Automatic root errors are suppressed after first presentation; manual review remains exhaustive. Valid recovered generations use their original exact-token suppression. |
| I4 failure retry loop | Resolved within the approved narrow scope. `RetryState::NeedsClock` arms a separate 30-second floor at the foreground completion clock. The timer constrains wake deadlines and checks readiness before automatic dispatch. Typing cannot reset it; success clears it. Error, panic, queue rejection, and cancelled failed protection all enter the floor. Explicit consumed association/selection intents are not automatic retry loops. |
| I5 released-lease fixture race | Resolved without weakening active-owner assertions. Bounded helpers retry only released-fixture acquisition or scan settlement; active contention still uses raw immediate APIs. Process tests continue to use finite owned children and kill/wait cleanup. |
| I6 gate evidence | Resolved for this source snapshot. The source manifest, command records, exit codes and independently verified log hashes establish which gates ran. Earlier incomplete/intermediate evidence is retained rather than represented as final success. |
| M1 unrelated contextual work cancelled | Resolved. Picker cancellation removes manual queues/requests/offers only. Exact request lookup ignores stale manual outcomes; contextual work survives. Manual completions are prioritized so a deferred automatic offer cannot strand a Loading picker. |
| M2 directory reclamation | Explicit design deferral remains appropriate. Stable lock inode/tombstone reclamation would require a separate safe ownership protocol; this change makes no cleanup claim for those directories. |
| M3 duplicate helpers | Resolved through shared owner validation and `fsx::canonicalize_with_missing_suffix`; errors remain fail-closed and foreign paths remain nonlocal. |
| M4 unnecessary body copy | Resolved. Discovery consumes borrowed record bodies and retains only bounded candidate metadata. Preparation alone creates the owned body while transferring the held source lease. Legacy owning-parser callers remain compatible through `parse_borrowed`. |
| M5 readability | Revised flow/picker structures and touched functions are hand-wrapped; no formatter or broad style-only churn was introduced. No remaining substantive code-quality finding. |
| M6 expected retention warning | Resolved. `RetainedByRule` yields informational detail; IO, identity corruption, interrupted cleanup, and uncertain post-unlink durability remain warnings. |
| Prior Codex F1 retired-source focus | Remains resolved. Exact-token protected-document focus precedes source preparation, makes no new request/Open event, and leaves failed/unprotected retry revalidation intact. |

## Cross-module safety assessment

The Save As amendment changes association eligibility only. Cleanup still holds the
originating slot's worker guard, validates owner, generation ceiling, version and exact
checkpoint body, compares complete saved bytes and fingerprint through one opened target
handle, syncs that handle and its parent, and immediately consumes the receipt. A cleanup
panic remains secondary to the completed user save. Newer/divergent checkpoints are retained;
stale ordinary-save and replaced-slot foreground merges retain their guards.

The original handoff proof remains intact: exclusive source lease, initial recovered
snapshot captured before Open callbacks, successor strict checkpoint and ancestor barriers,
Pending-to-Retiring CAS, and source unlink in one FIFO operation. Neither retry backoff nor
foreground detachment creates historical cleanup authority. Legacy imports remain copy-only.
Explicit close/Discard cancellation still differs from timeout detachment and cannot route
late protection metadata into replacement slots.

Automatic discovery and first-save suggestions remain nondestructive. Startup and runtime
opening use worker discovery; recovery installs a separate dirty pathless ordinary document.
Dismissal preserves records, manual review stays available, and cleaner protection includes
open/pending source aliases with identity failures refusing cleanup. Shared normalization
and borrowed parsing preserve the caps and same-handle/no-follow checks previously reviewed.

## Validation and limits

The verified coordinator evidence records **2,641 passed / 7 ignored**, successful workspace
build and no-run gates, and clean `cargo clippy --offline --workspace --all-targets`.
The terminal log ends exactly: **`smoke: 9/9 PASS`**. `validation.json` additionally records
successful `git diff --check`. These are inspected coordinator results, not reviewer-run
tests. The regression coverage includes delayed background success, quit detachment and
late-error isolation, Save As eligibility/refusal and strict receipt failures, unavailable
row/manual interaction, contextual scan preservation, retry floor timing, cancelled failures,
and the existing process/CAS fault matrix.

Linux runtime evidence does not establish Windows compilation or runtime durability.
Process-death tests do not simulate power loss. Foreground quit timeout does not interrupt
filesystem IO or bound the FIFO worker's shutdown join. Stable tombstone reclamation,
uncooperative external writers, and broader checkpoint scheduling remain documented follow-ups.
The final Fable re-review is a separate pending gate; its authentication availability does
not change this independent source verdict.
