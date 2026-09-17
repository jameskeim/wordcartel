# Recovery safety — final review complete

Merged into `main` on 2026-09-17 from `fix/recovery-safety` (base `9e409c0`).
Implementation commit: `b53973e`. [Merge verification](2026-09-17-recovery-merge.md).
Both final gates pass on the refined 45-file snapshot:

- [Codex polish review](2026-09-15-recovery-polish-implementation-review.md): GO, zero findings.
- [Fable polish review](evidence/2026-09-15-recovery-polish/fable-review.md): GO, zero findings.

The preceding full-scope reviews and the scoped design/implementation history remain
preserved. The user-approved M-A/M-D refinements changed four files; both reviewers
checked their effects on the existing recovery invariants. The coordinator rechecked
all 45 live and isolated source hashes after Fable restored its probe wiring.

## Validation

- Full workspace: 2,643 passed, seven ignored.
- Workspace build and compile-only tests: warning-free.
- Workspace/all-target clippy and `git diff --check`: clean.
- Recovery suite: 160 passed; Fable's six additional compiled probes passed.
- Mandatory terminal result: **`smoke: 9/9 PASS`**.

Exact commands, gate-log hashes and source-manifest identity:
[evidence manifest](evidence/2026-09-15-recovery-polish/validation.json).
Scoped checklist: [polish ledger](2026-09-15-recovery-polish-progress.md).

## Non-blocking follow-up checklist

M-A and M-D were folded into this effort at the user's request and independently
re-reviewed. M-B and M-C remain separate, non-blocking follow-ups.

- [x] M-A — Cancelled opening batches now reflect a matching durable successor Ack in
  the live buffer. Slot/path/generation checks, captured-version accounting, source
  retirement cancellation and late-status/quit suppression remain intact. Verified
  by regression matrix and independent compiled probes.
- [ ] M-B — Stable owner-lock directories and retained sources/successors have no general
  in-app reclamation policy. A failed import followed by Save As can leave its source
  available again next launch. This is conservative retention; any future reclamation
  design must preserve lease/inode stability and explicit ownership authority.
- [ ] M-C — A contextual offer arriving after its originating buffer becomes inactive is
  dropped. Manual Review Recovery Files still exposes it. Consider deferring the offer
  until the user returns if a later UX effort changes this approved behavior.
- [x] M-D — Metadata-less busy rows now say "Recovery files in use" and explain that
  an editor is using the files. Paths remain inspectable, known names are retained,
  and rows remain unavailable without inventing ownership or record metadata.

## Boundaries

Windows compilation/runtime durability remains unvalidated; Linux gate results do not
establish Windows support. Process-crash probes are not power-loss simulations. The quit
timeout revokes foreground waiting but does not bound OS IO or worker joining. Legacy
sources remain copy-only. The user-approved 30-second failure retry floor does not resolve
all other R3 scheduling questions.

Implementation, documentation and reviews are complete. The user authorized commit
and merge; the merged workspace tests pass. No push was performed.
