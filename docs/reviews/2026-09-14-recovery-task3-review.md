# Recovery Task 3 independent review

Date: 2026-09-14. Scope: design revision 5 with the contextual-provenance
implementation clarification; plan revision 2 Task 3; actual
`recovery_discovery.rs`, its child tests, and
`recovery_regressions::discovery_integration`. Inspected the supporting codec,
legacy parser and relevant RealFs handle/lock/directory APIs. Task 2 has a separate
review gate. This reviewer ran no cargo commands and changed no production code.

**Spec/plan compliance: GO for Task 3.**
**Code quality: GO for Task 3.**
Open findings: **0 Critical / 0 Important / 0 Minor.**

## Confirmed properties

- Scans resolve the trusted configured root without provisioning missing storage.
  Enumeration retains raw filenames and has no silent candidate cap. Root errors
  propagate; recognized corrupt, unsupported, oversized and unreadable candidates
  remain disabled rows rather than appearing recoverable or disappearing silently.
- V2 paths require the exact root/owner/checkpoint layout, validated owner syntax,
  private real directories and an existing stable lease. Leaf reads use a validated
  regular no-follow handle. Directory/metadata owner disagreement is rejected.
  These checks preserve the documented trusted-ancestor boundary; they do not claim
  protection against malicious replacement of trusted ancestors.
- Each scan reads one capped record, derives bounded Unicode preview/metadata and
  releases its lease before examining the next owner. It does not retain all bodies.
  Busy owners remain unavailable manual rows. Contextual scans omit those rows;
  the later automatic-offer consumer must also omit busy rows from all-scope scans.
- The codec enforces independent metadata/body caps, exact framing and validated
  timestamps. Legacy swaps use their own header, including named and unnamed work;
  panic dumps remain body-only candidates. Legacy filesystem time is explicitly
  distinguished from checkpoint civil time. Valid empty bodies remain recoverable.
- Contextual matching prefers current association and falls back to provenance only
  when association is absent. This is now explicit in the spec clarification and API
  comment. It grants neither a destination nor deletion authority. Relative missing
  filenames and missing normal ancestor components resolve against an existing
  ancestor; permission failures and unresolved parent traversal are not invented away.
- Preparation binds the selected v2 owner to its exact source path, reacquires the
  source lease and rechecks generation. Legacy preparation reopens the validated
  raw path and compares length/mtime/full-body hash from the same opened handle.
  Changed tokens return a refreshed candidate requiring reselection. Legacy identity
  remains explicitly best-effort, consistent with the design's concurrent-writer limit.
- Successful preparation moves the held lease with the body. Open/read/decode
  errors and changed-selection outcomes release ownership through RAII. No scan or
  preparation path removes, renames or rewrites a source.
- Missing checkpoints are classified while holding the owner lease. Inactive
  lock-only tombstones are omitted; active owners, temporary payloads and unreadable
  directory contents remain visible as unavailable. A previously selected source
  that became a tombstone produces a preparation error, not an empty import.

## Review observations resolved during this task

The initial current-association-only comment disagreed with the provenance fallback.
The parent clarified the engineering rule in the spec and corrected the API comment;
an independent fixture proves association B takes precedence over provenance A.
The initial missing-relative-filename normalization gap was corrected and tested.

The parent independently reproduced an empty-owner tombstone appearing as a scan
error. The final classification runs under the lease and distinguishes no payload
from incomplete temporary data. The archived red log preserves the failing fixture.
Additional independent fixtures now prove changed-v2 reselection, ownership release
after failures, ownership transfer on success and exact-cap/error distinctions.

## Evidence and limits

Read `evidence/2026-09-14-recovery-implementation/task3-tombstone-green.log`:
**18 passed, 0 failed, 0 ignored**, comprising 12 child tests and six parent-authored
integration fixtures. Inspected those test bodies and the final production source.
The reviewer did not independently execute the suite, perform a Windows runtime
test, or simulate power loss.

The initial clippy snapshot reported transitional unused preparation ownership and
consumer plumbing. This verdict does not describe the whole feature as wired or
the final workspace as warning-free. Foreground request lifecycle, automatic offers,
selection UI, save receipts, successor checkpoint/handoff and final end-to-end
validation belong to subsequent tasks and are not certified by this bounded gate.
