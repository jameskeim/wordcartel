# Recovery safety — independent Codex spec review, round 2

Date: 2026-09-14. Static review of technical proposal revision 2 against source
at `9e409c0`, including `app::run`, `app::finish_iteration`, both job-application
wrappers, `ThreadExecutor`, `plugin::fire_event`, and the e2e loop. No cargo,
source edits, or delegation.

**Design compliance: NOT READY. Quality: NOT READY. Gate: NO-GO.**

Revision 2 resolves the substance of round-one I1–I4 and M1–M2: strict save
cleanup, combined FIFO handoff, terminal failure ownership, format-specific tokens,
selection dismissal, and artifact status. Two Important integration gaps and one
Minor consistency issue remain. No Critical findings.

## Important I5 — The initial handoff dispatch boundary remains ambiguous relative to quit and plugins

B says foreground results enqueue follow-up intents, drained after plugin callbacks
and before the final quit barrier. E instead describes foreground acceptance as
installing the Buffer and firing Open, then requires combined handoff dispatch
before subsequent saves/checkpoints. The exact installation/dispatch seam is not
specified. An implementation that installs in a `JobResult::merge` and defers its
handoff dispatch until the advertised post-callback hook permits quit re-drive or
plugin Open commands to dispatch a save first.

This is a real distinction in current source: `jobs_apply::apply_job_outcome` and
`apply_job_result` call `drive_quit_drain` immediately after merge; `app::run` then
runs `PluginHost::pump`; `app::finish_iteration` runs afterward. Its current
`if !editor.quit { return true; }` also makes it unsuitable as an unconditional
ordinary recovery-intent drain unless explicitly moved. `plugin::fire_event`
only queues an event: installing in the post-pump hook instead delays Open until
another iteration and needs an explicit wake/service contract.

**Correction:** choose one exact production/harness choreography. A straightforward
choice is: prepare merge queues an install intent; both job-application wrappers
drain it with Ctx before quit re-drive; that drain installs, captures, dispatches
the combined transaction, then queues Open. Keep an unconditional post-callback
drain for commands/open intents emitted by plugins, ahead of finish_iteration's
early return. Specify how any events emitted there receive a prompt pump without
unbounded reentrancy. The implementation plan must map both e2e paths to those
same seams. Test plugin Open save/quit and a quit drain already awaiting another
save when a prepare completion arrives.

## Important I6 — A timeout detaches the UI wait, but cannot guarantee subsequent quit completes

F says a five-second timeout detaches a request from exit blockers and a subsequent
quit may proceed, while a worker retains its guards through completion. In the
actual `jobs::ThreadExecutor::drop`, dropping the executor unconditionally joins
the worker thread. A hung filesystem operation therefore still blocks shutdown
after the UI exit barrier, possibly with terminal restoration dependent on drop
order. Cancellation CAS cannot interrupt a blocked write/sync operation.

The current scope explicitly excludes broad FIFO worker redesign. The spec must
not silently promise bounded exit that this executor cannot supply. Nor should a
new implementation detach the worker and claim safe shutdown without addressing
later queued saves and cancellation guarantees.

**Correction:** explicitly delimit the timeout contract to foreground wait/quit
cancellation and acknowledge the existing blocking-join limitation, with a visible
policy that avoids an unsupported bounded-exit claim; or specify a narrow, reviewed
shutdown change that makes that claim true while preserving dispatched save duties.
Distinguish delayed completion, returned IO error, and indefinitely blocked IO in
the evidence requirements. This is a feasibility limitation, not a request to
simulate physical IO cancellation with a request flag.

## Minor M3 — The diagram and panic/failure rows overstate which operation and copy survive

G still labels `OpenUncheckpointed --> Protected` as "successor checkpoint or save
durable", although B now expressly forbids ordinary-save source retirement and
routes every handoff through the combined checkpoint transaction. F also says
"Combined checkpoint failure/panic ... keeps the original". A panic or error after
source unlink (for example during directory sync) may leave only the already-durable
successor. G's generic source-retirement-fails row similarly says source+successor
even though the prose correctly recognizes uncertain deletion after unlink.

**Correction:** label the protection edge as strict successor checkpoint; split
pre-retirement write failure from post-authorization retirement error/panic. Before
retirement, source is guaranteed; after source removal begins, successor is
guaranteed and source may also remain. Foreground panic handling should not report
the recovered document as unprotected when a strict successor may have committed:
report uncertain completion conservatively and preserve all remaining artifacts.

## Next gate

Resolve I5/I6 and M3, then re-review the revised spec. No approved product decision
needs to change to settle these contracts. The combined worker transaction and
strict RecoverySaveReceipt are substantially stronger than revision 1; retain
their FIFO and no-stale-deletion premises explicitly in the implementation plan.
