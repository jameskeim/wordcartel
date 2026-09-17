# Recovery safety — independent Codex spec review, round 3

Date: 2026-09-14. Static review of revision 3 against source at `9e409c0`.
Read the complete revised proposal, checked the actual startup/executor sequence,
result wrappers, plugin-event queue, shared exit hook, and libc lockfile version.
No cargo, source edits, or delegation.

**Design compliance: one Minor gap. Quality: one Minor gap. Gate: NO-GO pending
the small correction required by the project's clean-spec gate.**

No Critical or Important findings. Round-two I5, I6, and M3 are resolved: initial
handoff dispatch now precedes quit re-drive and plugin callbacks, timeout promises
are limited to foreground waiting, and failure reporting distinguishes protection
from retirement. The safe-open/platform limitations are stated without claiming
that leaf checks protect against malicious ancestor replacement.

## Minor M4 — Explicitly bootstrap startup discovery before the first blocking receive

D says startup emits an assessment intent and performs one all-candidate scan.
B names `recovery_flow::after_callbacks` in `app::finish_iteration` as the scan-intent
dispatcher. But `app::run` reaches its first `msg_rx.recv_timeout` before calling
that hook. With clean startup, no splash, and no user input, no dispatched recovery
job exists yet to provide the worker wake. Depending on other deadlines, discovery
can be delayed until unrelated input or a long idle timeout.

**Correction:** explicitly dispatch the startup all-candidate intent once after
the executor and wake relay are ready and before the first blocking receive.
Reuse the scan-intent dispatcher; do not install recovered buffers or pump plugin
events in startup construction. Add a bootstrap test that observes one scan
dispatch with no input/Tick, and confirms subsequent settled iterations do not
redispatch it.

This is a narrow integration omission, not a defect in the ownership/handoff
architecture. Once corrected, re-review the exact revision; the technical plan
should preserve all current gate and fault-test requirements.
