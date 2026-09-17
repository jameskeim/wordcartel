# Recovery polish ledger

User approved M-A cancellation-status accuracy and M-D busy-row clarity after the
prior recovery implementation received both final GO verdicts. M-B reclamation
policy and M-C deferred automatic offers remain separate. No commit/merge/push authorized.

Design and plan: [scoped amendment](2026-09-15-recovery-polish-design-plan.md).
Evidence: `evidence/2026-09-15-recovery-polish/`.

- [x] Ground the two changes against actual import/renderer code.
- [x] Reproduce both issues with failing behavioral tests.
- [x] Independent design/plan gate: GO, zero findings.
- [x] Implementation and focused green tests: 2 focused tests and 160 recovery tests pass.
- [x] Independent implementation spec/code gate: both GO, zero findings.
- [x] Workspace tests: 2,643 passed, seven ignored; build/no-run/all-target clippy/diff clean; `smoke: 9/9 PASS`.
- [x] Fresh source manifest and final Codex/Fable re-review: both GO, zero findings.
- [x] Record final verdicts; mark only M-A/M-D resolved in the follow-up checklist.

No source changes outside the scoped refinement are planned. Historical prior reports
and source evidence remain in the preceding effort's evidence directory.

Exactly four files differ from the previous 45-file reviewed snapshot. The complete
manifest and polish delta are recorded alongside hash-verified final validation logs.
Final Fable re-review and the final Codex manifest check are next.

## Final status

Complete. [Codex final addendum review](2026-09-15-recovery-polish-implementation-review.md)
and [Fable polish review](evidence/2026-09-15-recovery-polish/fable-review.md) both return
GO with zero findings. Fable independently reproduced the gates and ran six compiled
probes against the actual revised source. Its three observations require no addendum fix;
the retained-source observation belongs to the existing M-B cleanup deferral.

After Fable restored its probe wiring, the coordinator verified all 45 source hashes
in both the live and isolated checkouts. No source changes followed review. Final
validation: 2,643 passed, seven ignored; warning-free build/no-run/all-target clippy;
`git diff --check` clean; **`smoke: 9/9 PASS`**. Exact hashes and commands are in
[evidence/2026-09-15-recovery-polish/validation.json](evidence/2026-09-15-recovery-polish/validation.json).

Changes remain uncommitted. No merge or push was performed. M-B reclamation and M-C
deferred automatic offers remain separate follow-ups, as authorized by the user.

2026-09-17: user-authorized commit/merge completed as part of b53973e; merged-tree
verification passed 2,643 tests with seven ignored. See [merge verification](2026-09-17-recovery-merge.md).
No push performed. Earlier uncommitted-state entries describe their historical stages.
