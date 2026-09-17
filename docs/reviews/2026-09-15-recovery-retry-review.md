# Independent bounded retry review — 2026-09-15

Spec compliance: **GO**. Code quality: **GO**.
Open findings: Critical 0, Important 0, Minor 0.

Scope: the user-approved I4/R3 amendment in the final section of
`2026-09-15-recovery-fable-triage.md`, checked against the actual retry state,
checkpoint completion, import completion, Buffer initialization, timers, foreground
merge boundaries, timeout detachment, and `retry_integration` regressions. This is
a static scoped review, not the final whole-branch gate. No cargo commands or source
edits were performed by this reviewer.

The 30-second failure floor is independent of edit and successful-checkpoint
timestamps. Failure first marks `NeedsClock`; the foreground completion boundary
arms it exactly once. Slow worker execution therefore cannot consume the interval.
The automatic timer both constrains its deadline and checks readiness before
dispatch, while later typing cannot clear the floor. Queue rejection, ordinary
failure and routed worker panic share this behavior.

Fresh Buffer construction resets the floor with its fresh recovery slot. Exact
slot checks prevent old results from changing a replacement buffer. A successful
checkpoint clears the floor; the existing dirty/version gate then prevents writes
and timer wakes for settled content. Explicit association and recovery-selection
intents can dispatch independently of the automatic floor; they are consumed
once and do not form a failure-driven requeue loop.

Timeout detachment retains the outstanding request and its in-flight gate until
completion, avoiding a second queued attempt against an unresolved worker.
Detached late errors arm the floor without changing quit or status. Import
completion uses its retained durable Ack to distinguish checkpoint failure from
source-cleanup failure: the latter clears retry state because protection succeeded.

During review, cancelled completion skipped retry accounting. That permitted one
immediate automatic attempt after a cancelled failed operation, although the new
attempt's failure would have been delayed. The coordinator accepted extending the
floor to cancelled outcomes on the same live slot. Re-reading the revised ordinary
and import completion paths confirms this is fixed: failed protection arms the
floor, successful protection clears it, and cancellation still suppresses Ack
metadata/status/quit changes as before. The ordinary regression table now includes
the cancelled disposition alongside error and panic.

Evidence inspected: `evidence/2026-09-14-recovery-implementation/r3-retry-green.log`
reported two passing tests covering slow failure/panic completion, edits during
backoff, retry success followed by idle, and queue rejection. That log preceded
the final cancelled-outcome changes; the coordinator's rerun and final validation
must cover the revised snapshot. Static inspection of the revised cancellation
table and both completion paths found no remaining issue.

Evidence update: the coordinator subsequently reran `r3-retry-green.log` with the
cancelled disposition included; its final result is two passed, zero failed
(reported session 12091, exit 0). The final log was read by this reviewer. The
earlier evidence limitation above describes the initial review chronology only.

Reviewed source SHA-256:

```text
4b8257a607d905bd9af985f60aee91a0a4bee7fc0daf0be0d7f9686c7993fe9e  wordcartel/src/recovery_flow/retry.rs
0f79be59ce0b52b38df54818e1fede1f335ffb260db990f1e926e3def9a9b78e  wordcartel/src/recovery_flow.rs
233e5cf75a497757e809f75385709430bab9aa8639c4f4185d8c41d6fb0c34a7  wordcartel/src/recovery_flow/import.rs
b2603272bcafe748dc547ea8c22314d35e6c899f73aebb6b4bdaa27f818fbd87  wordcartel/src/editor.rs
b7239c747d40f26b44239be8d11b28c8e385c30e1831ad4ff1c96cb714334d24  wordcartel/src/timers.rs
b744687b90123260b5accefec87a3c62cef3b436700c5ccce1261b37f588aa8f  wordcartel/src/recovery_regressions.rs
```
