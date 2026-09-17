# Final independent Codex recovery review

Date: 2026-09-14. Baseline: `9e409c0f09d9683c40e84dbd2499c9f9e6e32ded`.
Reviewed the complete recovery-safety source snapshot in
`/tmp/wordcartel-fable-6ex3sk57/checkout`, including intent-to-add files,
against the approved design revision 5, implementation plan revision 2, and
implementation ledger. Snapshot identity is recorded by
`evidence/2026-09-14-recovery-implementation/source-sha256.json`.

**Initial verdict: NO-GO pending one Minor correction.**
Critical: 0. Important: 0. Minor: 1.
This verdict follows the agreed zero-unresolved-findings review gate; the finding
is a visible interaction defect, not a demonstrated loss-of-data defect.

## F1 — Minor: successful v2 recovery cannot be focused from its retained row

Symbols: `recovery_flow::import::drive`, `recovery_flow::import::install`,
`recovery_picker::batch_boundary`.

Trigger: select an inactive v2 candidate, let its combined handoff succeed, then
select that same original row again in the still-open picker. The footer explicitly
invites selecting a file to retry or focus. The handoff has removed the old source
checkpoint, but `drive` always calls worker `prepare` before the existing-document
lookup in `install`. Preparation therefore fails on the missing checkpoint; it
never reaches the live Buffer lookup. With several recovered documents, the user
cannot focus an earlier recovered document this way. No extra document opens and
no protected copy is deleted by the failed second action.

Evidence is the unconditional prepare call in `drive`, the post-prepare lookup in
`install`, and source unlink in that installation's combined handoff. The existing
`recovery_picker_multi_selection_imports_sequentially_and_focuses_live_legacy`
regression specifically covers a retained legacy source, so it cannot catch this
v2 difference. This is a deterministic source trace; this reviewer did not run a
probe or mutate production source.

Recommendation: before preparing a selected candidate, detect an exact-token live
Buffer that is already healthy/protected and focus it directly, without source IO.
Keep source reread/revalidation for failed or unprotected retries, and do not derive
new retirement authority from this focus path. Add a regression that successfully
retires a v2 source, switches to another Buffer, reselects its original row, and
asserts focus, unchanged Buffer count, no prepare dispatch, and preserved successor.

## Safety review conclusions

- Ownership is carried by the slot and stable lease inode, not original filename,
  BufferId, or PID. Exclusive owner creation and retained job guards prevent the
  original R1 overwrite/cleanup sharing. Failed attempts retain first-checkpoint
  ancestor-sync obligations, and generations are checked rather than reused.
- Acknowledgements follow file flush/sync, rename, strict owner sync, and first
  publication's full ancestor chain. Unsupported primitives return failure.
- Handoff keeps the exact source lease through successor publication and CAS
  authorization, and removes the source within the same FIFO worker operation.
  Pre-CAS cancellation preserves source; post-CAS cancellation cannot revoke the
  already durable successor. Legacy imports remain copy-only, including retries.
- Save cleanup checks owned generation, body, association and version, then proves
  saved bytes and fingerprint using the same opened target handle before file and
  parent sync. Optional cleanup errors/panics do not relabel the successful user
  write. Save merges guard editing-instance identity and stale ordinary paths.
- Checked recovery request identities route normal and panic completions; timed-out
  work detaches foreground exit blockers but retains safe completion routing and
  worker guards. Quit rejects preparations, cancels remaining selections, waits
  concrete handoff/association work, and preserves the post-callback dirty rescan.
- Runtime bootstrap and both successful open-install paths enqueue worker discovery;
  source records are preserved while scans are pending. Selector dismissal and
  subset handling are nondestructive. Cleaner protection includes open and pending
  source paths with alias normalization and refuses unverifiable identities.
- Record/body/header caps, lossless path tags, no-follow regular reads, private
  protocol-directory validation, and unavailable/busy classification are explicit.
  Startup/scans do not provision storage and do not retain all candidate bodies.

## Validation boundaries

Read-only independent review; no cargo commands or runtime probes were run by this
reviewer. Root owns full-workspace/build/clippy/smoke validation; Fable has an isolated
probe environment. Final gate results must be recorded separately before readiness
is claimed. I inspected the real subprocess boundary tests, fault matrices, scoped
regressions, opening journeys, and the threaded finite-release timeout regression.
Passing earlier scoped reviews was not treated as proof of cross-module safety.

Windows runtime durability remains unvalidated. Strict primitive failure preserves
copies rather than claiming acknowledgement. Process-death tests do not simulate
power loss. Foreground timeout does not interrupt filesystem IO or bound the FIFO
worker's shutdown join. Clean Save As may conservatively retain an old-associated
checkpoint as explicitly documented in the approved contract check. Checkpoint
scheduling and uncooperative external-writer races remain separate review efforts.

## Final F1 re-review

Date: 2026-09-15. **Final code-review verdict: GO.**
Remaining findings: 0 Critical / 0 Important / 0 Minor.
The initial finding and verdict above are preserved as review history.

Independently compared the live `recovery_flow/import.rs` and nested test file
against the isolated reviewed snapshot. `drive` now invokes `focus_protected`
before reserving or dispatching preparation. It requires an exact source token
match and either a clean document or a current-version checkpoint acknowledgement.
The shared `protected` predicate leaves failed/unprotected retries on the existing
source reread/revalidation route. Quit cancellation still empties the selection
queue before this path can run. Focusing changes no request, slot, generation,
retirement authority, or plugin Open event.

Inspected `task-final-f1-red.log`: the new retired-v2 regression fails on the old
implementation's unchanged active BufferId. Inspected `task-final-f1-green.log`:
20 handoff tests pass, including that regression, failed retry, cancellation,
queue rejection, and checkpoint/cleanup fault cases. The new test additionally
checks that focusing preserves a queued ordinary checkpoint and supports a clean
document without an Ack. No reviewer-side cargo run or source mutation was needed.

Reviewed correction hashes:

- `wordcartel/src/recovery_flow/import.rs`:
  `4c261e7074a3493f3864a1b4a6d0d4cd06c855b0ae54a78f8042a56bb8d10f01`
- `wordcartel/src/recovery_flow/import/tests.rs`:
  `bff7ddf091c1f6956160e0998dceae36f3058cb238717843a1e4cdb9095b2542`

This GO certifies the reviewed source plus this correction, and closes F1.
Coordinator final validation and the separate Fable gate remain required; this
review is not commit, merge, or push authorization.
