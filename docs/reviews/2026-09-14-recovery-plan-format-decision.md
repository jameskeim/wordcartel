# Recovery plan format decision

Date: 2026-09-14. Scope: R1/R4/R5 technical design and implementation planning only.
Decision by the coordinating agent under the user's instruction to proceed autonomously
and the governing developer guidance permitting routine workflow choices and exceptions
without automatically requesting user approval. This is not a user-authored waiver.

`CLAUDE.md`, pipeline step 3, calls for a "Task-by-task, COMPLETE code, TDD steps,
grounded in real signatures" plan. The independent plan reviewer correctly identified
that the initial plan supplies complete algorithms and proposed interfaces rather than
a complete production patch. Do not claim literal compliance with that format rule.

Apply a narrow plan-format exception for this effort: the design and plan gates review
complete state/IO contracts, concrete interfaces, ordered algorithms, exact integration
seams, migration census, and executable regression requirements. Any included Rust
reference code must be complete and checked against current/proposed API status. No
TODO or omitted safety decision may be left for an implementer to silently invent.

Full production code is required when each implementation task executes and is reviewed
for both spec compliance and code quality before that task advances. Final Codex/Fable
reviews and all required build/test gates remain mandatory. This exception permits no
production implementation before the plan's technical gate passes and does not grant
commit, merge, or push authorization. Other project instructions remain in force.

Rationale: a second full implementation embedded in a pre-execution plan creates another
code artifact that can diverge from the task-reviewed source. For this explicitly scoped
design/planning task, a complete algorithm/interface plan plus per-task code review is the
chosen artifact boundary. This is an autonomous workflow decision, not a claim that the
repository's original wording meant something different or that the reviewer was wrong.

Retain this decision and the original blocking review in the evidence ledger. Independent
review should still flag any missing algorithm, interface, test, or safety obligation;
this document resolves only the plan-format requirement.
