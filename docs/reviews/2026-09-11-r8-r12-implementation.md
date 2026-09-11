# R8 / R12 implementation report

Branch: `fix/quit-save-durability`.
Base revision: `d3aea924988f60702529f6c68a0e332e64d44f1d`.
Status: implemented and locally validated; uncommitted, not merged.
Policy: the user selected **Save all documents** for Save and Quit.

## Resulting behavior

Save and Quit preserves the active document's explicit Save/Save As step, then
saves every remaining dirty ordinary document. Before exiting, the quit driver
rechecks the live buffer set rather than trusting an exhausted old queue. This
includes documents edited or opened while another save was pending.

Review Each records explicit discard decisions for the buffer ID and version
actually displayed. Unchanged discarded versions are not repeatedly prompted;
later edits require a new decision. Review Save targets the reviewed buffer even
if focus changed. A stale review is displayed again. Cancel, timeout, conflict,
and rejected/failed save paths leave the editor running without a stranded quit.
Repeated quit requests cannot replace an existing awaited flow.

An ordinary save finishing after Save As changed the buffer's destination no longer
changes the current destination's saved version, fingerprint, checkpoint latch, or
recovery file. Its status names the destination actually written and warns that this
completion did not save the current document. Successful disk-write plugin events
remain truthful. Same-path stale-version saves and sequential Save As remain valid.

Post-save actions now bind to a unique save request. Buffer ID plus version alone
cannot distinguish two saves of the same version; the queued-close regression
exposed this during implementation. Only the exact awaited request can mark its
action ready. Errors or destination mismatches cancel their matching action rather
than inferring success from a previously recorded `saved_version`.

## Code organization

- [`quit.rs`](../../wordcartel/src/quit.rs): shared start, review, cancellation, and
  final dirty-set reconciliation.
- [`save.rs`](../../wordcartel/src/save.rs): destination guard, request identity,
  completion acknowledgment, explicit dispatch outcomes.
- [`editor.rs`](../../wordcartel/src/editor.rs): quit-flow decision state and
  request-bound pending action.
- [`jobs_apply.rs`](../../wordcartel/src/jobs_apply.rs) and
  [`prompts.rs`](../../wordcartel/src/prompts.rs): delegate to the shared flow and
  honor the correct completion. The legacy one-save quit action checks all buffers.
- [`durability_regressions.rs`](../../wordcartel/src/durability_regressions.rs):
  sixteen focused tests using actual transactions and deferred FIFO save jobs.

Other source changes are module registration and updates to existing test setup or
single-buffer completion expectations. No command IDs, keybindings, settings, or
menu/palette reachability changed. No rustfmt pass was run.

## Validation

| Check | Result | Evidence |
| --- | --- | --- |
| Original three regression requirements before implementation | All three failed for the expected defects | [Red log](evidence/2026-09-11-r8-r12-fix/red-tests.log) |
| Focused final regression suite | 16 passed | [Log](evidence/2026-09-11-r8-r12-fix/request-tests.log) |
| `cargo test --workspace` | 2,481 passed, 0 failed, 6 ignored across 18 summaries | [Log](evidence/2026-09-11-r8-r12-fix/workspace-tests.log) |
| `cargo clippy --workspace --all-targets` | Passed, warning-free | [Log](evidence/2026-09-11-r8-r12-fix/clippy.log) |
| `cargo build --workspace` | Passed, warning-free | [Log](evidence/2026-09-11-r8-r12-fix/build.log) |
| `cargo test --workspace --no-run` | Passed, warning-free | [Log](evidence/2026-09-11-r8-r12-fix/test-build.log) |
| Isolated PTY smoke suite | `smoke: 9/9 PASS` | [Log](evidence/2026-09-11-r8-r12-fix/smoke.log) |
| Final diff whitespace check | Passed | `git diff --check` |

The workspace suite includes the command-surface, filesystem-seam, and module-budget
guards. A first integration run exposed that the source-scanning filesystem guard
did not recognize a standalone test-only file; its tests now use the repository's
explicit `#[cfg(test)] mod tests` shape. Existing single-buffer assertions now drive
the shared quit continuation through the real completion entry point.

Full test/smoke runs used isolated XDG state/config/cache. Approved unrestricted
execution was needed for Unix socket fixtures/private tmux. No personal documents
were used. Ignored live-provider and benchmark tests were not enabled.

## Review and remaining work

Local diff review checked request identity, path/version ownership, discard
authorization, late completions after cancel/timeout, and cleanup preservation.
The same-version queued-save test prompted the request-ID refinement; the final
suite verifies that the earlier Save As cannot close a buffer on behalf of a later
ordinary save.

An [independent Fable whole-implementation review](2026-09-11-r8-r12-fable-review-summary.md)
has now completed: design compliance PASS, code quality PASS with Minor findings,
no Critical/Important findings, and conditional GO pending a legacy-code decision.
Separate independent pre-implementation design/plan reviews were not performed;
the final review is not a claim that those earlier gates occurred. The
[source patch](evidence/2026-09-11-r8-r12-fix/source.patch) and
[source hashes](evidence/2026-09-11-r8-r12-fix/source-sha256.json) remain the reviewed
snapshot. Nothing was committed, pushed, or merged.

R1–R7 and R9–R11 remain separate findings. In particular, this change does not solve
shared recovery filenames, missed recovery/checkpoint scheduling, or the external
write race. The original review reports continue to describe the baseline revision;
their defect-asserting probes are historical evidence, not acceptance tests for
this fixed branch.
