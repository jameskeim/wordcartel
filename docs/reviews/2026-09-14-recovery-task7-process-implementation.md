# Task 7 real-process recovery regressions

Implemented in `wordcartel/src/recovery_process_tests.rs`, registered only under `cfg(test)`.

Focused validation: `cargo test --offline -p wordcartel --lib recovery_process_tests::` passed **4 parent tests**, with **1 intentionally ignored helper**. Evidence: `evidence/2026-09-14-recovery-implementation/task7-process.log`. The helper runs explicitly in child test executables; the ignored result does not skip its coverage.

Coverage:

- Two simultaneous child processes each own a checkpoint for the same named target and one unnamed document. All four paths differ and remain busy while owners live. Killing and reaping both children makes all four exact bodies discoverable and preparable.
- A queued production `Job` closure retains the last `RecoverySlot` Arc after the original owner drops. Another process sees the lease busy until the queue is dropped without executing its job.
- One child holds a prepared source lease. A second child calls `prepare` using that source's previously selectable token and receives `WouldBlock`. Preparation succeeds after the first process is killed and reaped.
- The real production import/handoff runs in a child through `InlineExecutor`, with a test-only delegating Fs wrapper parking after temp write, temp file sync, rename, successor owner-directory sync, before source removal, after source removal, and after source-directory sync. Each owned child is killed and reaped at its marker. A fresh scanner and preparer then prove source or successor exact bytes remain; before retirement the source remains, after retirement the successor remains. Incomplete temporary records may appear as unavailable rows without hiding the valid source.

Each parent handshake and normal-exit wait has a 15-second deadline. Every child is owned by an RAII guard whose drop kills and waits. Parked children have a separate 45-second backstop, and the queue-release child has a 30-second backstop. No production hooks, global environment mutations, shell child trees, or source formatting changes were added.

These tests validate process death, lock release, and restart visibility on this Linux filesystem. They do not simulate power failure, storage-controller cache loss, or validate Windows runtime locking. Strict-sync failure tests and the existing Task 2 deterministic allocation-collision fixture provide complementary evidence; the independent-process ownership test does not force the random owner allocator to collide.
