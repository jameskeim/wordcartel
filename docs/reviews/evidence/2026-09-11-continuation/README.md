# Reproduction evidence

Reviewed source: `d3aea924988f60702529f6c68a0e332e64d44f1d`.

From the repository root:

```sh
python3 docs/reviews/evidence/2026-09-11-continuation/run-probes.py rerun.log
python3 docs/reviews/evidence/2026-09-11-continuation/run-sessions.py
```

The first command temporarily appends `probes.rs` as a test-only module to
`wordcartel/src/swap.rs`, runs only `review_probes` with one test thread, then
restores the exact original source. Do not edit that source concurrently. The
runner refuses to overwrite a concurrent change. It uses the repository's
existing Rust dependencies and test seams; no production fix is installed.

The second command temporarily installs an example from `session-probe.rs`, builds
it against the normal library, and runs two distinct live processes against the
same temporary document and isolated XDG state directory. It removes the temporary
example afterward. This avoids the unit-test state-directory override, which would
otherwise give each test process a different state directory and hide the collision.

PASS means an observed defect or stated positive control reproduced. These are
review probes, not tests asserting that the application is correct. Test names and
assertions distinguish defects from controls. The 64 MiB boundary case allocates
and writes a large temporary fixture; other cases are small and deterministic.

## Logs

- `original-three.log`: original three findings rerun.
- `recovery-paths.log`: eight tests adding open, recover, first-checkpoint, queued
  external-edit and Save As cleanup cases.
- `save-lifecycle.log`: eleven tests adding path-changing-save, named-owner and
  atomic-save failure cases.
- `boundaries.log`: thirteen tests adding palette and size-boundary cases.
- `final-probes.log`: initial modal probe failed because it incorrectly assumed
  the global earliest wake equaled the swap deadline. Reconcile had an earlier
  overdue deadline. The corrected assertion checks the swap deadline separately
  and that the global deadline produces zero timeout.
- `confirmed-probes.log`: corrected fourteen-test suite passed.
- `quit-paths.log`: final sixteen-test suite passed; matches retained `probes.rs`.
- `sessions.log`: real cross-process checkpoint collision.
- `smoke.log`: sandbox could not open private tmux sockets; all checks failed for
  that environment reason.
- `smoke-unrestricted.log`: approved retry, `smoke: 9/9 PASS`.

The 16-test suite uses `app::advance` after `reduce` where testing input paths,
so missing recovery deadlines are not artifacts of omitting that production stage.
Deferred-executor scenarios preserve FIFO ordering; they postpone execution rather
than inventing out-of-order completions. Fault tests use the existing `FaultFs`.

`build-coverage.py` regenerates `module-coverage.csv`: a complete file inventory and
partial lexical reference index. It is not a compiler-derived call graph. For the
actual baseline test/clippy logs and tool versions, see the sibling
`../2026-09-11-pilot/` directory.
