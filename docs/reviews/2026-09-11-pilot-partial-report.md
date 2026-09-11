# Wordcartel review pilot — partial report

Revision: `d3aea924988f60702529f6c68a0e332e64d44f1d`.
Scope: Pass 0 baseline and mapping, Pass 1 durability and lifecycle.
Disposition: stopped by the token budget; neither pass is claimed complete.

Continuation: Pass 0 and Pass 1 were subsequently completed on the same revision.
See the [baseline/map](2026-09-11-pass0-baseline-map.md) and
[durability review](2026-09-11-pass1-durability-review.md). The text below preserves
the earlier pilot's state and limits; it is not the current review disposition.

## Budget and process

The agreed ceiling was 30,000 tokens. The budget limiter reported 38,985 tokens
used when it stopped the pilot. The planned halfway checkpoint was missed; the
first explicit usage check already reported 24,861 tokens. Large source/tool
outputs made this review less token-efficient than intended. Future continuation
should use much smaller symbol-level reads and check usage after each batch,
reserving budget for evidence and reporting.

## Baseline evidence

- The reviewed revision is unchanged from the initial recovery review.
- Tracked source files were unchanged; pre-existing untracked scratchpad material
  was left alone.
- Indexed 115 Rust source files across the three crates in
  [modules.txt](evidence/2026-09-11-pilot/modules.txt).
- Recorded revision, tool versions, and initial working-tree status in
  [baseline.json](evidence/2026-09-11-pilot/baseline.json).
- The first workspace test run failed because the sandbox denied creation of a
  Unix socket fixture. This was an environment failure, not a code finding.
- After approved escalation, the full workspace test run completed successfully
  (2,465 passed, 0 failed, 6 ignored across 18 test summaries):
  [test log](evidence/2026-09-11-pilot/baseline-tests-unrestricted.log).
- Workspace clippy completed successfully before wrap-up:
  [clippy log](evidence/2026-09-11-pilot/baseline-clippy.log).
- Tests used isolated XDG state/config/cache directories under
  `/tmp/wordcartel-pilot`. No smoke tests, benchmarks, or live language-service
  tests were run in this pilot.

## Inspection coverage

Production implementations inspected during this pilot:

| Modules | Coverage |
| --- | --- |
| `fsx`, `file`, `pathx` | Atomic replace sequence, path resolution, bounded reads, error handling |
| `workspace`, `scratch` | Open/new/close/switch, scratch copy/move |
| `state`, `session_restore` | Persistence, resume, scratch restore, open into current buffer |
| `startup` | Configuration seeding and resume enablement |
| `swap` | Naming, cleanup, assessment, dispatch and completion; some output was truncated |
| `file_browser_commit` | Destination classification and commit routing; inspection incomplete |
| `editor`, `app`, `prompts`, `save` | Selected construction, recovery, timing, and cleanup paths only |

The 115-file index is an inventory, not evidence that all modules were inspected.
The full caller/ownership map and module-by-module coverage ledger remain unfinished.

## Findings and hypotheses

The [three prior findings](2026-09-10-wordcartel-recovery-findings.md) remain
supported by source inspection on the same revision. Their earlier probes remain
the reproduction evidence; those probes were not rerun during this pilot.

No additional finding was confirmed by a new probe. These follow-up hypotheses
must be verified before promotion to findings:

1. **Recovery on an in-session open:** `app::run` assesses swaps during launch,
   but inspected `workspace::open_as_new_buffer` and
   `session_restore::open_into_current` paths load text/resume state without an
   evident swap assessment. Trace all callers and test opening a crashed document
   through the picker while another document is already open.
2. **Recovered unnamed content may lose checkpoint protection:** the Recover
   prompt loads the body and deletes the orphan carrier. The replacement buffer
   starts with `last_edit_at = None`. Check the complete interceptor/epilogue path
   to establish whether any timer is armed before another edit. A second crash
   before saving is the relevant reproduction.
3. **First checkpoint during continuous typing:** deadline calculation derives
   its initial maximum interval from `last_edit_at` when no prior swap exists.
   Verify that real typing cannot keep moving both deadlines indefinitely; inspect
   the existing continuous-editing guardrail before adding a probe.
4. **Queued-save external modifications:** the foreground fingerprint check and
   worker write are separate. Use a deferred executor to test an external edit
   between dispatch and execution, respecting production FIFO ordering.

These are hypotheses, not four newly established bugs. No design changes were
approved and no production fixes or commits were made.

## Next step

Continue only with a newly agreed budget. First retain and rerun the three
original probes, then investigate hypotheses
1 and 2 using focused deterministic tests. Preserve runnable evidence before
expanding the module map. Pass 1's same-path/multiple-session, Save As cleanup,
failure matrix, and restart scenarios remain outstanding.
