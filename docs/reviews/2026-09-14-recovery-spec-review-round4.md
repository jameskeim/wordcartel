# Recovery safety — independent Codex spec review, round 4

Date: 2026-09-14. Reviewed technical proposal revision 4 against source at
`9e409c0`, prior complete-spec reads, the exact revision-3-to-4 delta, and the
independent recovery IO grounding report. No cargo, source edits, or delegation.

**Design compliance: PASS. Quality: PASS. Technical spec gate: GO.**

Open findings: **0 Critical, 0 Important, 0 Minor.** All findings from rounds 1–3
are resolved. This approves progression to the independently reviewed implementation
plan; it does not certify unwritten implementation or platform/runtime durability.

Revision 4 explicitly bootstraps the single startup scan after executor/wake relay
initialization and before the first blocking receive, resolving M4 without relying
on keyboard input or idle polling. The additional IO clarifications are consistent
with the architecture: shared leases require Send+Sync, save cleanup compares and
syncs the same handle, platform constants come from explicit target dependencies,
and foreground last-guard drop is limited to minimal handle closure.

The final design preserves all six approved product decisions. Its safety proof
rests on exclusive owner allocation, stable lock inodes, retained job leases,
strict checkpoint acknowledgement, a single FIFO checkpoint-and-retirement
operation, cancellation linearization, and immediate same-operation save cleanup
receipts. Legacy imports remain copy-only, and unselected/dismissed candidates
remain available. Initial handoff dispatch precedes quit re-drive and plugin Open
callbacks through the actual production job-wrapper seam.

The implementation plan must carry forward the documented limits and checks:
unsupported locks/sync preserve copies; Windows support needs cfg and runtime
evidence; leaf no-follow checks assume the stated trusted private hierarchy;
legacy content-change detection is observational; the five-second timeout does
not interrupt OS IO or bound the existing executor join. These are explicit scope
boundaries, not outstanding findings. Required fault, deferred-executor, real
process-lock/crash, command/overlay, and no-input bootstrap tests remain gates for
the corresponding implementation behavior.
