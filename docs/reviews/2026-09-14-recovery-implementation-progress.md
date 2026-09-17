# Recovery safety implementation ledger

Branch: fix/recovery-safety, based on main 9e409c0.

Current status: implementation and the user-approved polish are complete. Final
Codex and Fable reviews both return GO with zero findings on the refined snapshot.
See the [final summary](2026-09-15-recovery-final-review-summary.md) and
[polish ledger](2026-09-15-recovery-polish-progress.md). Changes remain uncommitted.

User authorized implementation after design revision 5 and plan revision 2 received
independent GO verdicts. Commit, merge and push are not authorized for this effort.

| Task | Status | Evidence / review |
| --- | --- | --- |
| 1 Strict IO, acceptance reporting and test support | Complete | Independent spec/code GO, zero findings; IO9, dispatch4, jobs8, save/quit33, source guards15 pass; workspace clippy clean |
| 2 Codec, ownership, strict checkpoints | Complete | Independent spec/code GO round2; store11 + integration5 + root alias1 pass; workspace clippy clean |
| 3 Discovery and legacy revalidation | Complete | Independent spec/code GO; 18 tests pass including source revalidation, faults, caps and tombstones |
| 4 Requests, checkpoints, save integration | Complete | Independent spec/code GO round2; flow8, cleanup11, save integration6, old durability33 pass |
| 5 Separate-document import and handoff | Complete | Independent spec/code GO; 13 handoff integrations, 2 Save As and 6 lifecycle/CAS tests pass |
| 6 Selector, command and opening paths | Complete | Independent spec/code GO, zero remaining findings; picker18, opening/cleaner5, diagnostics1, rendered journeys2, prompts42, swap37 pass |
| 7 Quit/process regression coverage | Complete | Independent spec/code GO after cancellation fix; quit8, subprocess4, flow25, timers9, jobs28, e2e69 pass |
| Final validation and Codex/Fable reviews | Complete, including polish | Workspace2643 passed/7ignored; build/no-run/clippy clean; smoke9/9; Codex GO and Fable GO, zero findings; M-B/M-C deferred |

Evidence directory: `docs/reviews/evidence/2026-09-14-recovery-implementation/`.
Sources remain uncommitted. Preserve unrelated scratchpad artifacts; no rustfmt.

Task 1 review: [independent report](2026-09-14-recovery-task1-review.md).
Task 1 source manifest: `evidence/2026-09-14-recovery-implementation/task1-source-sha256.json`.

Task 2 review: [round 1](2026-09-14-recovery-task2-review.md),
[final round 2](2026-09-14-recovery-task2-review-round2.md).
Store regression evidence includes an explicitly labeled post-implementation ancestor
sync mutation test. The initial compilation-red output was overwritten; task2-red.log
records that limitation and makes no fabricated TDD chronology claim.

Task 3 review: [report](2026-09-14-recovery-task3-review.md). Task 4 review:
[initial](2026-09-14-recovery-task4-review.md), [final](2026-09-14-recovery-task4-review-round2.md).
Task 4 clippy has no lint errors; private discovery handoff members remain temporarily
unconsumed until Task 5. Final build/clippy must be warning-free after integration.

Task 5 review: [report](2026-09-14-recovery-task5-review.md). Task 6 review:
[report](2026-09-14-recovery-task6-review.md). Its two display findings are fixed;
focused validation/re-review is pending. Task 7 is now implementing concrete quit
waits and deadlines, real subprocess ownership/crash tests, and plugin integration.

Task 7 grounding clarification: existing Lua event hooks are observer-only;
`plugin/api.rs` rejects edit and command calls from event hooks. The plan's literal
"callbacks editing/saving on recovered Open" cannot run under that contract.
Validation instead checks the real recovered Open observes the initial content,
rejects those mutations, and occurs after handoff dispatch; ordinary registered Lua
commands exercise allowed edits/saves. Existing plugin semantics remain intact.

Task 7 final review: [report](2026-09-14-recovery-task7-review.md). Recovery timeout
while a quit-owned Save As picker was open left a stale continuation; the fix clears
only quit-owned state, preserves CloseBuffer, and permits manual picker continuation.
Two regressions failed before the fix; all eight focused quit tests then passed.

Initial full workspace run: 2164 passed, three failures, two ignored. The socket fixture
was sandbox-blocked (its unrestricted rerun passed). Migrated the remaining obsolete
legacy-delete save assertion to owned-generation cleanup and made a dropped-source test
fixture tolerate the bounded fork/exec descriptor inheritance window. The real threaded
handoff barrier now invokes the actual timeout timer before finite worker release.
The full unrestricted workspace rerun is in progress; original failure evidence retained.

## Final review fixes and validation — 2026-09-15

See [Fable triage and user decisions](2026-09-15-recovery-fable-triage.md),
[implementation corrections](2026-09-15-recovery-fable-implementation-fixes.md),
and [independent retry review](2026-09-15-recovery-retry-review.md). Both newly
raised behavior decisions were explicitly approved by the user and documented in
the design/plan amendments. Original reports/probes remain under `fable-round1/`.

All required validation is now green: workspace 2641 passed, seven ignored; warning-free
workspace build and compile-only tests; workspace/all-target clippy; filesystem guard
10/10; five repeated parallel recovery suites (158 passed each, helper ignored in
each parent suite and explicitly invoked by process tests); `git diff --check`.
Mandatory terminal result: **`smoke: 9/9 PASS`**. Root source/test-helper updates
are complete. A new hash-verified snapshot will receive final Codex/Fable re-review;
there is no final GO claim until those reviewers return.

Windows compilation/runtime durability remains unvalidated. The 5-second quit wait
only revokes foreground waiting; ThreadExecutor still joins outstanding worker IO.
Process-crash tests prove restart behavior, not simulated power loss. Legacy sources
remain copy-only and stable owner-lock tombstone reclamation remains explicitly deferred.
The approved 30-second failure retry floor does not claim to solve other R3 scheduling issues.
No commit, merge, push, or unrelated scratchpad changes have been performed.

Final Fable re-review launch on the revised 45-file snapshot failed before any model
work: "Failed to authenticate: OAuth session expired and could not be refreshed".
The user was asked to run `claude auth login`; the external gate remains pending.
Auth failure evidence is preserved in `fable-round2-auth-failure/`. The report at
`fable-round1/fable-review.md` is historical; no final Fable verdict exists yet.
Independent Codex re-review continues against the revised, validated source.

Final Codex round 2: **GO**, zero Critical/Important/Minor findings. The reviewer
verified all 45 source hashes and five gate-log hashes against the revised design
and explicit user decisions. Report: [final Codex review](2026-09-15-recovery-final-codex-review.md).
The only outstanding gate is Fable re-review, currently blocked by expired Claude
authentication. Implementation and all local validation are complete; changes remain
uncommitted on `fix/recovery-safety`.

Claude authentication was refreshed by the user. The resumed Fable re-review is
running against the same verified 45-file snapshot; no source changes occurred.

Final Fable round 2: **GO** after refreshed authentication. All blocking findings are
resolved; four design-conformant/non-blocking Minor observations are recorded in the
[final review summary](2026-09-15-recovery-final-review-summary.md). Both independent
gates now pass. Post-review verification found zero mismatches across all 45 live and
isolated source files. Final report/probes/events are preserved under `fable-round2/`.
No source changes, commits, merges or pushes followed this review.

User-approved M-A/M-D polish is complete and independently re-reviewed. Both final
gates return GO with zero findings; post-review hashes match all 45 source files.
Updated validation and remaining two follow-ups are in the linked final summary.
