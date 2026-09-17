# Recovery polish design/plan review — 2026-09-15

Design: **GO**. Implementation plan: **GO**.
Findings: Critical 0, Important 0, Minor 0.

Independently cross-checked `2026-09-15-recovery-polish-design-plan.md` against
`recovery_flow/import.rs` (installation, completion, cancellation), `progress.rs`,
the store Ack construction, `recovery_picker.rs`, discovery busy-row construction,
and automatic/manual scan filtering. No cargo commands or production edits were made.

M-A is supported by the current interfaces: the exact removed request retains its
slot, captured path/version and durable Progress Ack. Moving valid Ack bookkeeping
outside the cancellation guard remains safe when the existing slot, path and
generation checks remain. Recording the captured version preserves protection
accounting for subsequent edits. This requires neither worker IO changes nor renewed
source-retirement permission; cancellation's CAS and the final suppression of late
status/quit effects remain separate. The planned negative tests cover wrong slots,
paths/generations and unsuccessful checkpoints.

M-D fits the current metadata-only rendering surface. `Candidate.busy` already
identifies lease contention, unavailable rows cannot be selected, and automatic
offers filter busy rows. A readable generic name and explanation can therefore be
rendered without reading files or asserting which editor owns a lease. Known
association/provenance names and inspectable paths can be retained with the existing
path helpers. Geometry and command wiring need no change.

The plan is confined to the two accepted items, leaves reclamation and delayed
offers deferred, and includes behavioral red tests, focused implementation review,
fresh validation and both final independent gates. Implementation may proceed.
