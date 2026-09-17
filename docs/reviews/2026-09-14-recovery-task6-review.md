# Recovery Task 6 independent review

Reviewed against approved design revision 5 and implementation plan revision 2 on
`fix/recovery-safety`. This is a focused Task 6 gate, not the final feature gate.

**Spec/plan compliance: GO. Code quality: GO.**

Final findings: **0 Critical / 0 Important / 0 Minor**. Both original Minor
findings below have been fixed and the revised source inspected. Coordinator confirmed
Task 6 e2e evidence green; the final focused display/picker evidence was independently
read and reports 18 passed, zero failures. The `imports_busy` migration preserves
the picker/discovery import-specific busy predicate as Task 7 adds separate exit blockers.

## Original findings, now addressed in source

- **M1 — Foreign-platform original paths disappear from the selector.**
  `wordcartel/src/recovery_picker.rs:142` calls `TaggedPath::local_path()` and falls
  back to the checkpoint storage path when it returns None. A valid Windows-path
  checkpoint reviewed on Unix therefore shows its owner storage path rather than
  the known original filename/path. Design A explicitly preserves foreign paths
  for display; D3 requires original filename where known. Use the tagged escaped
  representation for this case, as the recovered buffer display already does.
  This does not grant a save target or affect recovery safety.
- **M2 — Long paths permanently hide timestamp and availability.**
  `wordcartel/src/recovery_picker.rs:149` concatenates a full path before timestamp
  and status; lines 159–164 render it on one clipped row while the detail area
  contains only body preview. The maximum list width is 86 cells. An ordinary
  long directory path can consume that width, hiding the entire checkpoint time
  and error/busy explanation even on a large terminal. Multiple candidates for
  the same document then cannot be distinguished using the required time display.
  Allocate bounded space for filename/time/status or provide highlighted-row
  metadata details. Add rendered coverage with long names and unavailable rows.

The revised implementation extracts a foreign tagged filename and retains its
escaped full representation. Rows allocate independent filename, time and status
fields; highlighted details separately show path, timestamp, availability and body
preview. Left/Right scroll the full path without changing selection, while ordinary
row navigation resets path offset. Painting and hit testing still use the same
geometry. The added actual-render tests cover foreign original names and a long
path with visible checkpoint date and busy explanation. Their red evidence exists
in `task6-display-red.log`; `task6-display-green.log` records all 18 picker/discovery
tests passing, including both new display regressions.

## Source assessment

- Runtime bootstrap occurs after ThreadExecutor and completion wake relay creation,
  before the first blocking receive. It schedules an all-candidate scan without
  installing buffers or pumping plugins. Named startup uses the same path.
- Both successful runtime disk installation branches enqueue assessment; file
  picker and recents converge on those branches. Constructors remain pure.
  Discovery normalizes association through the injected filesystem on the worker.
  Late results cannot replace disk content; stale contextual origins do not
  interrupt another document and remain manually discoverable.
- `finish_iteration` calls recovery after callbacks unconditionally before its
  non-quit return and quit callback check. Both harness step variants mirror the
  boundary, and harness outcome application uses production wrappers. Prepared
  installation remains in `after_job`, preceding callback and quit re-drive.
- Registry supplies the File-category manual command without a default binding.
  Overlay table, painter, mouse, interceptor and common close delegate are wired.
  Completion messages pass the input interceptor. Safe automatic offers defer
  behind incompatible overlays; manual requests visibly refuse active save/close
  flows. Empty, scanning and scan-error states have distinct messages.
- Selection starts empty; zero acceptance remains open; subset acceptance records
  unselected exact tokens. All close routes share suppression and epoch invalidation.
  Only closing an Opening picker cancels imports; closing a scan/loading picker
  leaves unrelated installed handoffs alone. Changed candidates require selection
  again, and repeat live legacy recovery focuses the existing document.
- Geometry is shared by painting and hit testing, including scrolling and tiny
  terminal sizes. Body preview is capped in discovery, and civil checkpoint time
  is separate from explicitly labeled legacy filesystem mtime.
- Old replacement/deletion prompt actions, staging fields and `load_recovered`
  API are removed. Recovered imports preserve disk diagnostics and install fresh
  diagnostics state. Save paths no longer invoke legacy path-derived cleanup.
- Cleaner protection includes open document paths, queued/running import sources,
  open recovered sources and v2 storage. Both list and confirmation recompute the
  protection set. Canonical alias matching uses the supplied Fs; inability to
  establish identity aborts cleanup visibly. No new discovery/render raw-Fs bypass
  was found. The pre-existing legacy state-directory provisioning is unchanged in
  scope and is not a new recovery worker implementation.

## Validation evidence

No cargo commands or production/test source edits were performed by this reviewer.
Inspected coordinator logs under
`docs/reviews/evidence/2026-09-14-recovery-implementation/`:

- `task6-picker-green.log`: 16 passed.
- `task6-display-green.log`: 18 passed after both display fixes (supersedes the
  original 16-test picker run).
- `task6-overlays-green.log`: 26 passed.
- `task6-registry-green.log`: 82 passed.
- `task6-open-clean.log`: 5 passed.
- `task6-diagnostics.log`: 1 passed.
- `task6-prompts.log`: 42 passed.
- `task6-swap.log`: 37 passed.
- `task6-e2e.log`: 2 passed, including rendered startup without a keypress and
  actual file-picker/recents selection through separate recovery installation.

These are scoped tests and static integration inspection. Task 7 quit blockers,
timeout transitions, real process/crash tests, full workspace gates and independent
whole-branch review remain pending. This report does not assert feature completion,
power-loss guarantees, or bounded shutdown under indefinitely blocked OS IO.
