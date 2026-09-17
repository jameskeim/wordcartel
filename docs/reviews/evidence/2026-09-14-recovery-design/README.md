# Recovery design evidence

Baseline: main at `9e409c0f09d9683c40e84dbd2499c9f9e6e32ded`.
`baseline.json` records toolchain and source hashes. No product code was changed.

`python3 docs/reviews/evidence/2026-09-14-recovery-design/run-probes.py baseline-probes.log`
runs five selected historical R1/R4/R5 defect assertions. All five pass, confirming
these defects still reproduce; this does NOT mean recovery is fixed. The runner
appends a test-only module to swap.rs, uses isolated state, and restores its exact
original bytes in finally. Run only with no concurrent edits of swap.rs.

`lock-probe.rs` checks the pinned std file-lock API: a child process cannot acquire
a held lock, can acquire it after release, and local directory sync succeeds. Compile
with `rustc --edition=2021 --deny warnings --crate-name recovery_lock_probe lock-probe.rs
-o /tmp/wordcartel-recovery-lock-probe`, then invoke with a nonexistent lock filename
in a new temporary directory. `lock-probe.log` records the result. It proves only
Linux/local-filesystem primitive feasibility, not cross-platform or power-loss safety.
The first compilation omitted --edition=2021 and emitted format-string warnings;
recompilation with the repository edition and --deny warnings was clean.

`spec-revision*.md` preserve the proposals reviewed in earlier rounds. The live spec
and independent findings reports are outside this evidence directory. Raw design
source inspection is documented in the source and IO grounding reports.
