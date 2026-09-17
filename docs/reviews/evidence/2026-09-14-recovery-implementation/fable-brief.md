# Independent whole-change recovery safety review

Review the exact uncommitted recovery-safety snapshot against the actual code and approved design/plan:
- docs/superpowers/specs/2026-09-14-recovery-safety-design.md
- docs/superpowers/plans/2026-09-14-recovery-safety-plan.md
- docs/reviews/2026-09-14-recovery-implementation-progress.md

Inspect cross-task ownership, strict durability acknowledgements, source retirement CAS/leases,
Save As receipt cleanup, cancellation/timeouts/quit final barriers, legacy preservation,
selector/command opening flows, and resource/IO boundaries. Check regressions against HEAD.
source.patch includes new source files. Source hashes and validation evidence are in
 docs/reviews/evidence/2026-09-14-recovery-implementation/.

This checkout is isolated. You may compile focused probes and run tests here; save probes
and outputs under review-probes/. Never modify the user's working tree. Do not commit,
push, merge, or contact any human. Do not run cargo fmt/rustfmt. Treat CLAUDE.md's pipeline
as context for this final reviewer role, not a request to launch more agents or redo planning.

Write FABLE_REVIEW.md with GO/NO-GO, findings classified Critical/Important/Minor, exact
symbols/trigger/evidence and suggested fixes, probe commands/results, and limitations.
Review independently; do not assume passing tests or earlier scoped GO proves correctness.
Windows runtime durability is unvalidated; process death tests do not simulate power loss.
ThreadExecutor still joins on shutdown; foreground timeout does not bound OS IO/process exit.
Lua event hooks remain observer-only; commands exercise allowed callback edits/saves.

## Second review round

The first report, probes, and exact source snapshot are preserved under
`docs/reviews/evidence/2026-09-14-recovery-implementation/fable-round1/`.
Review all revised source and the cross-task effects of fixes, not only old probes.
Read `docs/reviews/2026-09-15-recovery-fable-triage.md` for finding dispositions and
explicit user approval of strict Save As receipt eligibility and a bounded automatic
checkpoint failure retry delay. The design/plan amendments record these decisions.
The stable owner-lock/tombstone reclamation policy remains a documented design deferral.
No Windows durability claim is supported by this Linux-only validation.

The original snapshot copied gate logs while final verification was still running.
The coordinator's earlier clippy invocations DID include --workspace --all-targets, but
the cfg(test) wrapper edits happened after those runs. Dev-profile stdout alone does
not establish which target flags ran. Verify the actual final commands/snapshot rather
than inferring flags from that stdout. Final gate evidence is captured after fixes.
