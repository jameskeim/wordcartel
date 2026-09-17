# Task 7 independent review

Review and fix re-review of the uncommitted `fix/recovery-safety` implementation against the
approved recovery design revision 5 and plan revision 2. This review reads the
actual request, quit, timer, application, plugin and process-test code. No cargo
commands or production source edits were performed by the reviewer.

**Final Task 7 spec compliance: GO. Final Task 7 code quality: GO.**

Zero outstanding Critical, Important or Minor findings. The initial verdict was
NO-GO for I1; the finding and its verified resolution are retained below.

## Findings

### I1 — Important, resolved: cancelling recovery during quit-owned Save As retained quit ownership

`quit::cancel` clears `quit_drain`, provisional `quit`, the picker owner and an
awaited save, but leaves `pending_save_as == ContinueQuitDrain`. A recovered
pathless document can enter Save All while its handoff is pending, opening the
quit-owned filename picker. The recovery timer or handoff failure then calls
`quit::cancel`. The UI reports that quit was cancelled, but `quit::in_progress`
still returns true because of that retained continuation. Manual Save As remains
blocked and the existing picker has lost the owner needed to proceed as a quit
save. An overwrite confirmation can retain the same stale continuation.

Anchors: `wordcartel/src/quit.rs::cancel`, `quit::in_progress`,
`recovery_flow/pending.rs::timeout_tick`, `recovery_flow/import.rs::finish`.

The fix clears only the quit-owned Save As continuation in `quit::cancel`,
preserving unrelated CloseBuffer continuations and already-dispatched writes.
The remaining picker or overwrite confirmation may continue as an ordinary
manual Save As. Re-read the revised function and both new regression tests.
`task7-quit-picker-red.log` records both regressions failing against the old
function: stale `in_progress`, and an overwrite submission dispatching no save.
`task7-quit-picker-green.log` records all 8 focused quit tests passing after the
fix. The tests cover timeout, failure and panic during the actual quit picker,
CloseBuffer preservation, and a confirmed overwrite reaching disk without
resuming quit. I1 is resolved.

## Reviewed evidence and boundaries

- Concrete exit blockers distinguish dispatched handoff/association work from
  scans, preparations, ordinary checkpoints and persistent protection flags.
  Checked request identities remain registered through detachment for safe late
  routing; the timer removes each deadline once and attempts cancellation through
  the shared CAS. It does not detach a thread or claim to interrupt filesystem IO.
- Both job-outcome funnels run recovery progression before quit progression. The
  shared callback boundary cancels preparations before final dirty-buffer checks.
  Discard keeps version-specific decisions while cancelling pending retirement.
- Existing plugin event hooks reject edits/commands. The documented adjustment
  tests a real observer-only Open callback, queued initial handoff, and an allowed
  plugin command editing and invoking Save. Existing real plugin quit-rescan
  journeys remain in the e2e suite.
- Legacy-symbol census shows no `JobKind::SwapWrite`, staged swap-body fields,
  same-buffer recovery loader or old recovery prompt actions. The remaining
  `dispatch_swap_write` is a v2 dispatch wrapper; legacy parser/cleaner APIs and
  compatibility tests remain deliberate.
- `task7-process.log` records 4 passing parent tests and 1 deliberately ignored
  subprocess entry. The child entry is explicitly executed by parents. Reviewed
  tests cover independent named/unnamed owner processes, queued ownership,
  direct `prepare` contention, process death and seven production handoff crash
  boundaries. Owned child cleanup is bounded and uses kill plus wait. This
  establishes Linux process-death visibility, not power-loss simulation or Windows
  runtime validation. Deterministic allocation collision remains covered by the
  Task 2 fixture rather than random process collisions.
- `task7-e2e.log` records 69 passed and 1 intentionally ignored benchmark;
  `task7-quit-green.log`, `task7-flow-green.log`, `task7-timers-green.log` and
  `task7-jobs-green.log` record the focused pre-fix evidence. Whole-workspace and
  final independent review gates remain separate coordinator work.

The deferred-worker timeout test and real worker post-CAS cancellation test
currently exercise separate seams. Combining timeout with a finite real worker
release would strengthen evidence for the plan's explicit timeout/release case;
this is an evidence improvement, not a second observed production defect.
