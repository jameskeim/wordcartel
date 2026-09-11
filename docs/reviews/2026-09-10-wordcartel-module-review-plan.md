# Wordcartel module review plan

Date: 2026-09-10  
Initial baseline: `d3aea924988f60702529f6c68a0e332e64d44f1d`  
Scope: `wordcartel`, `wordcartel-core`, `wordcartel-nlp`, and their integration boundaries.

## Objective

Find reproducible defects in correctness, durability, responsiveness, and resource
handling. Review individual module contracts and the interactions that can violate
them. Produce evidence-backed findings and an explicit coverage record, not a
general assurance that the application is bug-free.

Start with the three [existing recovery findings](2026-09-10-wordcartel-recovery-findings.md).
They establish risks to investigate, not the limit of this review.

This is a review plan. Production fixes, design changes, commits, and merges are
separate work. Findings that conflict with an approved design must present both the
design requirement and the observed behavior for a decision.

## Working method

1. Record the commit and working-tree state before each pass. Keep the review on a
   stable revision. Claude Code is currently idle; no separate checkout is needed
   solely to avoid concurrent edits. If implementation resumes, use an isolated
   checkout for probes and record which revision each finding describes.
2. Read `CLAUDE.md` and the relevant contracts/spec sections to establish intent.
   Check claims against source and callers; comments and previous review verdicts
   are context, not proof. Anchor notes on symbol names plus the reviewed revision.
3. Enumerate source modules with `rg --files`. Maintain a coverage ledger mapping
   every module to a pass, its direct callers, and review depth: unread, mapped,
   inspected, or exercised. Assign utility modules to their primary consumer;
   record exclusions explicitly so small modules are not silently skipped.
4. For each module, document ownership, identities, invariants, entry points,
   state transitions, error paths, and expensive operations. Trace callers that
   can invalidate its assumptions, including plugins and asynchronous delivery.
5. Check existing tests before adding probes. Use small deterministic reproductions
   for concrete hypotheses. Assert user-visible invariants rather than duplicating
   implementation details. Keep speculative risks separate from confirmed defects.
6. Preserve probe source, commands, expected/actual results, and fixture setup with
   each confirmed finding. Remove temporary source instrumentation after use.
   Probes asserting a defect should be clearly distinguished from future tests
   asserting corrected behavior.
7. Publish a pass report before moving on. Report severe findings promptly; do not
   wait for the entire review to finish or silently broaden into implementation.

Claude's familiarity may help resolve specific questions about design intent or
previous approaches. Record those questions and, if consultation is available,
cross-check its answers against source and tests. No consultation has occurred as
part of this plan; it is not a prerequisite or substitute for independent evidence.

## Review sequence

### Pass 0 — Baseline and coverage map

**Work:** map the crates, module ownership, command dispatch, edit flow, buffer
lifecycle, background jobs, and external processes. Create a diagram of the main
state owners and message paths, plus the module coverage ledger.

**Validation:** run the workspace baseline commands below and distinguish failures
already present from failures introduced by probes. Inspect ignored tests and
external-tool requirements before selecting additional checks. The prior review's
2,361 passing library tests are historical evidence, not a substitute for the full
baseline on the revision used for this review.

**Deliverable:** baseline report, test/environment limitations, module ledger, and
an initial list of cross-module invariants.

### Pass 1 — Durability, recovery, and buffer lifecycle

**Modules:** `save`, `swap`, `recovery`, `file`, `fsx`, `pathx`, `editor`, `workspace`,
`scratch`, `state`, `session_restore`, `startup`, and file-browser commit paths.

**Questions:**

- Can one buffer or process overwrite or delete another's saved/recovery content?
- Are document identity, buffer identity, chosen path, resolved path, and version
  distinguished at every write and completion boundary?
- Does every dirty buffer eventually receive a checkpoint, including inactive,
  recovered, unnamed, and programmatically edited buffers?
- Are external modifications detected across open, save, Save As, and reload?
- Does cleanup preserve the only recoverable copy after a failed or partial save?
- Can quit/close proceed while unsaved edits or required writes remain?

**Scenarios:** reproduce all three existing findings; add Save As cleanup, two
buffers sharing a named path, two sessions, symlink destinations, external edits
during queued saves, recovery followed by a crash, and continuous typing before
the first checkpoint. Inject create/write/flush/sync/rename failures through `Fs`.

**Deliverable:** durability findings, an identity/ownership table, and a matrix
showing which content survives each tested failure and restart.

### Pass 2 — Job delivery, timers, and shutdown

**Modules:** `jobs`, `jobs_apply`, `timers`, `app`, `panicx`, `term`, plus job
producers and lifecycle consumers identified in Pass 1.

**Questions:** stale or duplicate results, closed targets, cleared in-flight flags,
pending actions, retry policy, worker failure, queue growth, and shutdown waits.
Identify synchronous work that can block the input loop or delay durability jobs.

**Scenarios:** dispatch work for A; edit, switch, reload, or close A before delivery;
then deliver success, error, or panic. Exercise both the deterministic executor and
the production worker. Preserve the worker's real FIFO guarantees when constructing
tests; distinguish achievable races from hypothetical future concurrency.

**Deliverable:** a producer-to-completion table, terminal-state checks for each job
kind, and evidence that settled/failing states either block or retry deliberately.

### Pass 3 — Editing, selection, and history

**Modules:** core `buffer`, `change`, `selection`, `history`, `textobj`, `register`,
`search`; shell `transact`, `edit_apply`, commands, `blocks_marked`, `marks`,
`scratch`, `search_ui`, `transform`, and `ventilate`.

**Questions:** atomic edits, valid Unicode boundaries, selection mapping,
read-only enforcement, coalescing, undo/redo, dirty-state semantics, history eviction,
cross-buffer operations, and invalid or stale external edits.

**Scenarios:** empty documents, multibyte/combining text, multiple selections,
overlapping ranges, end-of-file edits, replace operations, undo after save, redo
after branching, and memory-budget eviction. Check cross-buffer move behavior when
either destination or source rejects an edit.

**Validation:** extend existing property tests only where an invariant is missing;
use the `apply_pipeline` fuzz target for uncovered edit-boundary hypotheses.

**Deliverable:** edit invariants, minimized counterexamples, and missing coverage.

### Pass 4 — Parsing, derived state, navigation, and layout

**Modules:** core `block_tree`, `md_parse`, `outline`, `layout`, `count`, `style`;
shell `derive`, `reconcile`, `fold`, `nav`, `lines`, `compose`, `block_paint`, and
render consumers.

**Questions:** incremental soundness, cache keys and generations, stale background
reconciliation, fold anchors, cursor visibility, geometry, and document-size costs.

**Validation:** build on `wordcartel-core/tests/block_tree_oracle.rs`, the existing
render integration tests, and the `block_tree` fuzz target. Compare incremental
results with full recomputation across edit chains, undo, reload, and buffer switches.
For deliberately bounded-stale results, verify the documented temporary behavior
and eventual reconciliation; do not demand immediate full-parse equality where the
contract explicitly permits lag.

**Scenarios:** nested lists, fences, headings, long lines, Unicode display widths,
folded selections, tiny terminal sizes, and repeated edits before reconciliation.

**Deliverable:** cache dependency map and reproducible parser/layout discrepancies.

### Pass 5 — Plugins, language services, and external tools

**Modules:** `plugin/*`, `diag_provider`, `diagnostics_run`, `diag_overlay`,
`lsp_rpc`, `lsp_client`, `harper_ls`, `ltex_ls`, `clipboard`, `filter`, `export`,
`nlp`, `lenses`, core `diagnostics`, and `wordcartel-nlp`.

**Questions:** input validation, request ownership, cancellation, stale versions,
exactly-once completion, callback limits, observer restrictions, reload teardown,
subprocess lifetime, framing/output limits, and diagnostic byte-range conversion.
Establish the actual plugin trust model before interpreting access as a defect.

**Scenarios:** malformed/oversized messages, delayed diagnostics after reload or
close, provider death, hung tools, output floods, callback errors, timer cascades,
and plugin reload with queued work. Prefer controlled fake peers for deterministic
protocol tests; label live-service results with tool versions and availability.

**Deliverable:** boundary contracts, failure-cleanup coverage, and confirmed violations.

### Pass 6 — UI contracts and responsiveness

**Modules:** `input`, `keymap`, `registry`, `prompts`, overlays, `search_overlay`,
`mouse`, `menu`, `palette`, `settings`, file-browser modules, `status`, rendering,
themes, cursor controls, and remaining utilities in the coverage ledger.

**Command-surface contract:** assess conformance to
[`docs/design/command-surface-contract.md`](../design/command-surface-contract.md):
registry ownership, exhaustive palette access, menu subset, shared option setters,
and hints resolved against the active keymap. This review changes no commands.

**Scenarios:** overlay cancellation, delayed results while a modal is open,
read-only actions, resized/tiny terminals, custom bindings, selection replacement,
buffer switches, and recoverable errors that must remain visible.

**Performance:** use the existing release typing benchmark in `e2e.rs`. Record
document size/structure, build profile, machine, and p50/p95/p99 timings where the
harness exposes them. Measure idle wakeups and filesystem operations separately;
investigate allocations and scaling where measurements suggest a problem. Compare
against documented budgets; avoid inventing a universal latency threshold.

**Deliverable:** UI journey results, command-contract findings, and measured
responsiveness/resource findings with practical limits stated.

### Pass 7 — Cross-module synthesis

Revisit the highest-risk scenarios across completed passes: edit → checkpoint →
Save As → switch → close → restart; plugin edit → undo → diagnostics delivery;
incremental parse → fold → background reconcile; failed save → quit timeout → retry.

Check each finding against its actual trigger and current reviewed revision,
deduplicate shared root causes, and distinguish defects from intended tradeoffs.
Audit the coverage ledger for unread modules and untested critical boundaries.

**Deliverable:** final prioritized report, module coverage, retained probes,
validation summary, unresolved design questions, and an ordered remediation list.

## Validation commands and isolation

Baseline, once per reviewed revision unless a failure or change warrants repetition:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

Use targeted suites during each pass. At synthesis, run the PTY smoke suite and
quote its exact summary, including skips/failures, as advisory evidence:

```sh
scripts/smoke/run.sh
```

Performance pass:

```sh
cargo test -p wordcartel --release e2e_bench -- --ignored --nocapture --test-threads=1
```

Inspect fixture behavior before execution. Unit tests have a test-state-directory
redirect, but integration tests and real binaries do not inherit `cfg(test)`.
Use temporary documents and isolated XDG state/config/cache directories for probes
and smoke runs that could otherwise touch personal recovery or configuration data.
Do not use real user documents as crash-test fixtures. Do not run `cargo fmt`.
Fuzzing is targeted and bounded; record target, seed/corpus, duration, and minimized
failure. Passing tests or a finite fuzz run establish only the exercised coverage.

## Reporting and completion criteria

Use one report per pass under `docs/reviews/`, with supporting probes and logs in a
dedicated review evidence directory. Each finding includes:

- Stable review-local ID, severity, confidence, and reviewed revision.
- Affected symbols, reachable trigger, expected behavior, and actual behavior.
- Concrete impact and reproduction evidence, or an explicit unverified label.
- Suggested remediation and a regression scenario without silently approving a design.

Prioritize P1 data loss/corruption or serious hangs, P2 functional/resource defects,
and P3 minor behavior/documentation issues. Reserve P0 for demonstrated urgent,
broadly applicable failures. Keep review-local IDs separate from backlog IDs.

A pass is complete when its modules and immediate boundaries are inspected,
critical scenarios are exercised or explicitly recorded as blocked, and findings
have enough evidence to reproduce or assess. A clean result must state scope and
limits. Blocked scenarios remain coverage gaps, not implicit passes.

The review is complete when all passes have reports, every source module has a
coverage disposition, and the final synthesis identifies unresolved risks and
remediation priorities. Completion does not require fixing findings. Any later fix
follows the repository's implementation/review workflow and reruns the relevant
reproduction plus required gates on the changed revision.
