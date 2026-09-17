# Recovery safety — independent Codex spec review, round 5

Date: 2026-09-14. Reviewed revision-5 amendments against the previously GO
revision-4 architecture, current `jobs::Executor`/implementations, Cargo.lock,
and the locally cached rustix API source. Static review only; no cargo, source
edits, or delegation.

**Spec compliance: PASS. Quality: PASS. Technical design gate: GO.**

Open findings: **0 Critical, 0 Important, 0 Minor.**

The first-checkpoint full ancestry-sync requirement explicitly survives failure,
unpublished allocation, and new-slot retries. It no longer infers durability merely
from a directory's existence. Clearing the obligation only after complete success
resolves the plan-review provisioning gap; the corresponding fault regressions
must still demonstrate these retries.

Required `try_dispatch` supplies the acceptance result absent from current source.
Keeping `dispatch` as a default historical fire-and-forget wrapper preserves callers
while requiring every implementor to provide real acceptance behavior. Recovery
request insertion precedes possibly immediate execution, and rejection is an exact
terminal failure rather than an outstanding wait. This is a bounded trait/caller
migration without changing FIFO execution or shutdown guarantees.

The explicit Unix rustix dependency and safe
`rustix::process::geteuid().as_raw()` call are supported by the cached 1.1.4 sources
and current lockfile. They avoid an unsafe libc call for owner validation.

All previously documented platform, filesystem, legacy, and blocking-join limits
remain in force. This review clears the amended specification; the revised
implementation plan requires its own gate. The separately documented plan-format
decision concerns artifact format and does not alter these technical guarantees.
