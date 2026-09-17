# Recovery safety design progress

Scope: complete technical design and independent design/implementation-plan reviews
for R1/R4/R5, preserving approved decisions D1–D6. Production implementation has not
started. No new commit, merge, push, or goal budget change is authorized by this ledger.

## Evidence and decisions

- Baseline: main 9e409c0; tracked source unchanged after probe restoration.
- Five defect-asserting R1/R4/R5 probes reproduced successfully on this baseline.
- Independent source grounding and IO feasibility reports completed.
- Linux lock contention/release and directory-sync feasibility probe passed.
- Design round 1: NO-GO, 4 Important + 2 Minor. Resolved receipt strength,
  atomic handoff, terminal lifecycle, selection identity, dismissal and status.
- Design round 2: NO-GO, 2 Important + 1 Minor. Resolved job/plugin/quit ordering,
  foreground timeout vs worker-join limitation, and panic failure semantics.
- Design round 3: 0 Critical/Important; 1 Minor startup bootstrap omission.
- Revision 4 supplies the bootstrap and incorporates IO feasibility details;
  final independent design review passed: GO, 0 Critical / 0 Important / 0 Minor.
- Implementation plan round 1: NO-GO, 3 Important + 1 Minor. Technical corrections:
  retain/reconstruct full first-checkpoint ancestry sync obligations across failures;
  report queue rejection explicitly; use safe effective-UID validation.
- Coordinating agent documented a scoped plan-format exception to the full-code-plan
  rule; complete interfaces/algorithms/tests are planned, actual code is reviewed per
  task. This is not a user-authored waiver or literal compliance claim.
- Design revision 5 independently reviewed those technical clarifications: GO, zero findings.
- Implementation plan revision 2: independent GO, zero findings.
- Cancellation reference compiled with Rust 2021 and --deny warnings; both CAS-order
  tests passed. Other proposed APIs are not claimed compiled/implemented.
- Final verification: HEAD remains 9e409c0; grounded source hashes match and tracked
  source has no diff. All work here is uncommitted documentation/evidence.

## Completion

Technical design and reviewed implementation plan are complete. Seven implementation
tasks remain future work, with per-task spec/code reviews and final Codex/Fable gates.
No product changes were necessary to the six user-approved decisions.

## References

- [Approved decisions](2026-09-11-recovery-safety-brainstorm.md)
- [Technical design](../superpowers/specs/2026-09-14-recovery-safety-design.md)
- [Source grounding](2026-09-14-recovery-grounding.md)
- [IO grounding](2026-09-14-recovery-io-grounding.md)
- [Round 1](2026-09-14-recovery-spec-review-round1.md)
- [Round 2](2026-09-14-recovery-spec-review-round2.md)
- [Round 3](2026-09-14-recovery-spec-review-round3.md)
- [Final design gate](2026-09-14-recovery-spec-review-round5.md)
- [Implementation plan](../superpowers/plans/2026-09-14-recovery-safety-plan.md)
- [Initial plan review](2026-09-14-recovery-plan-review-round1.md)
- [Final plan gate](2026-09-14-recovery-plan-review-round2.md)
- [Scoped plan-format decision](2026-09-14-recovery-plan-format-decision.md)
- [Evidence](evidence/2026-09-14-recovery-design/README.md)
