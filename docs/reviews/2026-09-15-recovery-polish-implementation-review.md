# Recovery polish independent implementation review — 2026-09-15

Spec compliance: **GO**. Code quality: **GO**.
Findings: Critical 0, Important 0, Minor 0.
Scoped final Codex re-review: **GO**, extending the previous full review for the
four changed files below. Final validation and Fable evidence remain separate gates.

## Scope and source identity

Reviewed the approved `2026-09-15-recovery-polish-design-plan.md` against the actual
import completion/cancellation/worker flow and picker rendering/selection, including
the new regression bodies and related Save cleanup, retry and discovery interactions.
Read the previous full Codex GO report. Independently compared the live files with
all 45 hashes in `evidence/2026-09-15-recovery-polish/previous-final-review/source-sha256.json`:
exactly these four files differ. Their reviewed SHA-256 values are:

```text
f2362956739e1ce4c25eac9ff3acc10a55d9469edf18f00f6946ba8cce61d300  wordcartel/src/recovery_flow/import.rs
76b5102a7c6efabdc85eb2677b70f217d75ff8dc47ac619a45d8abadc4e84748  wordcartel/src/recovery_flow/import/tests.rs
0eab9e8bef0e02b7d72ad4e1ec44a9bbd700736142ede743f8eddfb4fc42460e  wordcartel/src/recovery_picker.rs
1b72cbac6d978a8ed82d754844144caa3994c14f5ca67e9e5c6f6c1d0e767c24  wordcartel/src/recovery_picker/tests.rs
```

No cargo invocation or production-source edits were performed by this reviewer.

## Findings and cross-task checks

M-A is resolved in `import::finish`. The removed exact request must still identify
the live Buffer's slot; its captured path and Ack generation must match before
protection metadata changes. Cancellation no longer suppresses a valid durable Ack,
including one retained in `Progress` through a later worker panic/cleanup failure.
`swapped_version` records the captured version, so edits made in flight remain
unprotected. Only valid Acks clear prior protection failure. No Ack is synthesized
from `Progress::protected()` or from cancellation.

The worker's source lease, strict successor checkpoint, `record_ack` and retirement
CAS are unchanged. The new foreground bookkeeping cannot authorize deletion or
repeat a source cleanup. Cancelled and detached results still return before picker
error reporting, status updates or quit cancellation. Retry accounting continues
to distinguish failed protection from a successful durable checkpoint with failed
source cleanup. Save receipt ownership/generation/content checks remain unchanged.

M-D is resolved in `paint_row` and `paint_details`. Metadata-less busy rows lead
with "Recovery files in use" and explain the restriction in ordinary language;
known association/provenance filenames retain the existing path presentation.
The metadata-less source path remains inspectable in the details area. Neither
message identifies the owning editor/session or claims that a checkpoint exists,
is absent, or represents a clean document. Rendering adds no IO or metadata lookup.
The existing unavailable/token selection guards and automatic busy-row filter
remain intact; geometry, path scrolling, commands and dismissal are unchanged.

M-B reclamation and M-C delayed offers remain deferred; neither is changed or
represented as resolved by this addendum.

## Evidence inspected

In `evidence/2026-09-15-recovery-polish/`:

- `focused-red.log`: both new tests failed on the intended missing Ack and busy-label
  behaviors before implementation.
- `focused-green.log`: both tests passed after implementation.
- `recovery-green.log`: 160 passed, zero failed, one ignored.

The Ack matrix covers a real successor checkpoint with preserved source after
cancellation, later edits, path/generation mismatch, closed/replaced Buffer slots,
missing Ack, retained Ack through late panic, cancelled remaining selection and
unchanged late status/quit. The busy test renders the real picker, checks readable
labels and inspectable source, and drives Space/Enter to verify no open request.
The existing known-path test also verifies its filename and scrollable original
path while showing the revised explanation. Existing tiny-terminal/mouse tests
are included in the broader recovery run.

These are inspected implementer/coordinator logs. The final workspace/build/no-run/
all-target clippy/smoke and snapshot evidence will be recorded by the coordinator;
this scoped source GO does not claim that those in-progress gates have completed.

## Final Codex addendum gate

**Final Codex verdict: GO.** Open findings remain **0 Critical / 0 Important /
0 Minor**. This completes the Codex addendum gate for the unchanged reviewed source;
Fable's final re-review remains independent and is not covered by this verdict.

Independently verified every live source hash against the final 45-file manifest,
whose SHA-256 is
`7c711c530da16cb68a001734b3785035e448cb7b0cca1ae2d72f4ddd857a6a9e`.
Its file set matches the prior full-review manifest and exactly the four reviewed
files differ. Read the complete 146-line `polish-source.patch`; it matches the
reviewed narrow production changes and regression additions.

Verified the manifest hash and all five gate-log hashes against final
`evidence/2026-09-15-recovery-polish/validation.json`. All recorded exit codes are
zero. Independently summed the workspace result lines: **2,643 passed, zero failed,
7 ignored**. Workspace build, compile-only tests and workspace/all-target clippy
logs complete successfully without warning/error diagnostics. The inspected
terminal log ends exactly **`smoke: 9/9 PASS`**. Also ran the read-only
`git diff --check` independently; it exited zero.

This evidence update supersedes the earlier pending-validation statement above;
the chronology is retained. No reviewer cargo run, production edit, commit or merge
was performed. Existing Linux-only validation and filesystem-IO shutdown limits
remain unchanged.
