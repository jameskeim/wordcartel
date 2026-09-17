# Recovery safety Task 1 independent review

Date: 2026-09-14. Reviewer: independent `recovery_task1_review` agent.
Authority: recovery design revision 5 and implementation plan revision 2.
Scope: strict filesystem primitives, injected fault support, observable executor
acceptance, noncreating state-path resolution, and deterministic recovery executor
helpers. Source was inspected directly; this reviewer ran no cargo commands and
made no production or test edits.

**Spec compliance: GO. Code quality: GO.**
Open findings: **0 Critical / 0 Important / 0 Minor** for this task's primitives
and harness scope.

## Source checked

- `wordcartel/src/fsx.rs` and `fsx/recovery_io_tests.rs`: public documented handle
  traits; default Unsupported capabilities; exclusive creation; one nonblocking
  lock attempt; no-follow, nonblocking regular-leaf validation; effective-UID and
  private-directory validation; same-handle stat/read/sync; strict directory sync.
- `wordcartel/src/test_support.rs`: injected recovery failures, per-occurrence
  failure journal, strict-operation path journal, and retained legacy fault behavior.
- `wordcartel/src/jobs.rs`: both missing-sender and disconnected-sender rejection
  release the captured Job before returning Closed. Compatibility dispatch retains
  prior semantics; acceptance and worker panic remain distinct.
- `app.rs`, `e2e.rs`, `durability_regressions.rs`: all existing Executor wrappers
  implement the required acceptance method and preserve their prior behavior.
- `recovery_regressions.rs` and its test-only `lib.rs` registration: deferred FIFO
  execution uses production Job::execute, independently of foreground application;
  rejection explicitly drops ownership. Neither helper substitutes a different Fs.
- `swap.rs`: state_path resolves without creating; legacy state_dir still provisions
  its existing path exactly as before.
- Target dependencies and lockfile: exact approved Unix libc/rustix and Windows
  windows-sys dependencies; no unsafe code added.

## Review refinements resolved

1. The original FIFO test used an unjoined potentially blocked thread. It now uses
   a bounded child process, kills/reaps on timeout, and requires a completion-file
   handshake proving the selected helper actually tested both read and lock paths.
2. The initial operation journal lacked paths. Strict recovery operations now retain
   paths for later ancestor-chain assertions. Unsupported and Busy are independently
   asserted; reusable deferred/rejecting executors were added and inspected.
3. A focused run exposed transient post-drop lock contention while another test
   spawned a child. The post-drop assertion now allows only bounded WouldBlock
   retry; the held-guard Busy assertion remains immediate. This accommodates the
   fork-to-exec descriptor inheritance interval without hiding a permanent leak.

## Evidence and limits

Read the recorded behavioral red evidence and green results in
`evidence/2026-09-14-recovery-implementation/`:

- `task1-io-red.log`: three behavioral failures against unsupported scaffolding.
- `task1-dispatch-red.log`: rejection behavior fails against prior silent acceptance.
- `task1-io-green.log`: final focused run, 9 passed.
- `task1-dispatch-green.log`: 4 passed.
- `task1-jobs.log`: 8 passed; `task1-save-quit.log`: 33 passed.
- `task1-source-gates.log`: 15 passed across chokepoint and module-budget checks.
- `task1-clippy.log`: final workspace/all-targets clippy finished without warnings.

Older broad-IO logs precede the last harness refinement. This source verdict does not represent a
Windows compile/runtime durability claim. Windows branches are statically reviewed
only, and strict-sync failures must remain visible and preserve sources in later
integration.

Full ancestor provisioning/retained sync obligations belong to the Task 2 store
algorithm. Exact request cleanup, source-lock reacquisition after installed handoff
rejection, retirement CAS barriers, and foreground merge/quit behavior require the
later domain tasks. The helper and primitive review does not certify these
not-yet-implemented flows or the complete R1/R4/R5 feature.
