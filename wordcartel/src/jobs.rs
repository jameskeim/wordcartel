//! General background-job substrate (spec §4.1). Shell-only: the core stays
//! thread-free. One worker thread (production) gives FIFO result ordering for
//! free; `InlineExecutor` gives deterministic, thread-free tests.

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use crate::editor::Editor;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum JobKind {
    Save,      // one-shot, user-initiated: always applies
    SwapWrite, // one-shot housekeeping: always applies (status only)
    Reparse,   // coalescible background block-tree reconcile; version-checked in merge
    PosSweep,  // coalescible background POS sweep; version-checked in merge
    #[cfg(test)]
    CoalesceProbe, // test-only stand-in for a future coalescible kind
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultClass {
    /// Mutates buffer-local state (status/saved_version/cadence); dropped if the buffer is gone.
    BufferLocal,
    /// External filesystem side effect that must complete even if the buffer was closed.
    Durability,
}

/// A unit of background work, dispatched for a document version, run on a
/// worker, merged back on the foreground.
pub struct Job {
    pub buffer_id: crate::editor::BufferId,
    pub class: ResultClass,
    pub version: u64,
    pub kind: JobKind,
    /// Runs on the worker thread; must not touch the Editor directly.
    pub run: Box<dyn FnOnce() -> JobResult + Send>,
}

/// What a worker hands back: its own foreground merge effect.
pub struct JobResult {
    pub buffer_id: crate::editor::BufferId,
    pub class: ResultClass,
    pub version: u64,
    pub kind: JobKind,
    /// Applied on the foreground before the next draw. By contract this touches
    /// only non-document bookkeeping and DERIVED document caches (e.g.
    /// `document.blocks`, regenerable from text); any document-TEXT change must
    /// route through `editor.apply`.
    pub merge: Box<dyn FnOnce(&mut Editor) + Send>,
}

/// The outcome of a background job: either a normal result or a panic that
/// was caught by the executor boundary. Panicked outcomes carry the metadata
/// needed for per-kind cleanup (buffer stays dirty, swap_in_flight cleared, etc.).
pub enum JobOutcome {
    Done(JobResult),
    Panicked { buffer_id: crate::editor::BufferId, version: u64, kind: JobKind, msg: String },
}

/// Staleness now consults the result class + whether the buffer still exists.
pub fn is_stale(r: &JobResult, editor: &Editor) -> bool {
    match r.class {
        ResultClass::Durability => false, // must always complete
        ResultClass::BufferLocal => match editor.by_id(r.buffer_id) {
            None => true, // buffer closed -> drop the buffer-local merge
            Some(b) => {
                #[cfg(not(test))]
                let _ = b;
                match r.kind {
                    JobKind::Save | JobKind::SwapWrite | JobKind::Reparse | JobKind::PosSweep => false,
                    #[cfg(test)]
                    JobKind::CoalesceProbe => r.version != b.document.version,
                }
            }
        },
    }
}

pub trait Executor {
    /// Enqueue a job for the worker.
    fn dispatch(&self, job: Job);
    /// Non-blocking: collect any results ready now (consumes them).
    fn drain(&self) -> Vec<JobOutcome>;
}

/// Deterministic test executor: runs `job.run()` immediately on `dispatch`,
/// buffers the result for `drain`. No threads, no flake.
#[derive(Default)]
pub struct InlineExecutor {
    pending: RefCell<Vec<JobOutcome>>,
}

impl Executor for InlineExecutor {
    fn dispatch(&self, job: Job) {
        let (buffer_id, version, kind) = (job.buffer_id, job.version, job.kind);
        let outcome = match crate::panicx::catch(job.run) {
            Ok(result) => JobOutcome::Done(result),
            Err(msg) => JobOutcome::Panicked { buffer_id, version, kind, msg },
        };
        self.pending.borrow_mut().push(outcome);
    }
    fn drain(&self) -> Vec<JobOutcome> {
        self.pending.borrow_mut().drain(..).collect()
    }
}

/// Production executor: one worker thread, FIFO. The worker pushes each
/// JobOutcome onto an internal channel (drained by `drain`) and sends a unit
/// "wake" nudge on `wake` after each result so the main loop can wake and drain.
pub struct ThreadExecutor {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<JobOutcome>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadExecutor {
    pub fn new(wake: Sender<()>) -> ThreadExecutor {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<JobOutcome>();
        let worker = std::thread::Builder::new()
            .name("wcartel-jobs".into())
            .spawn(move || {
                // FIFO: process jobs in dispatch order. Exit when job_tx drops.
                while let Ok(job) = job_rx.recv() {
                    let (buffer_id, version, kind) = (job.buffer_id, job.version, job.kind);
                    let outcome = match crate::panicx::catch(job.run) {
                        Ok(result) => JobOutcome::Done(result),
                        Err(msg) => JobOutcome::Panicked { buffer_id, version, kind, msg },
                    };
                    if result_tx.send(outcome).is_err() { break; }
                    let _ = wake.send(()); // nudge the loop to drain
                }
            })
            .expect("spawn jobs worker");
        ThreadExecutor { job_tx: Some(job_tx), result_rx, worker: Some(worker) }
    }
}

impl Executor for ThreadExecutor {
    fn dispatch(&self, job: Job) {
        if let Some(tx) = &self.job_tx {
            // A send failure means the worker died; the next drain will surface
            // nothing and the UI stays responsive. Dropping the job is safe.
            let _ = tx.send(job);
        }
    }
    fn drain(&self) -> Vec<JobOutcome> {
        let mut out = Vec::new();
        while let Ok(r) = self.result_rx.try_recv() {
            out.push(r);
        }
        out
    }
}

impl Drop for ThreadExecutor {
    fn drop(&mut self) {
        // Drop job_tx so the worker's recv() returns Err and the loop exits.
        self.job_tx = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, BufferId};

    #[test]
    fn thread_executor_runs_job_on_worker_and_drains_result() {
        use std::sync::mpsc;
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let (wake_tx, _wake_rx) = mpsc::channel::<()>();
        let ex = ThreadExecutor::new(wake_tx);
        ex.dispatch(Job {
            buffer_id: BufferId(0),
            class: ResultClass::Durability,
            version: 7,
            kind: JobKind::Save,
            run: Box::new(move || {
                done_tx.send(7).unwrap();
                JobResult {
                    buffer_id: BufferId(0),
                    class: ResultClass::Durability,
                    version: 7, kind: JobKind::Save,
                    merge: Box::new(|e: &mut crate::editor::Editor| e.set_status(crate::status::StatusKind::Info, "worker")),
                }
            }),
        });
        assert_eq!(done_rx.recv().unwrap(), 7, "worker must run the job");
        let mut results = Vec::new();
        while results.is_empty() { results = ex.drain(); }
        assert!(matches!(&results[0], JobOutcome::Done(r) if r.version == 7));
    }

    #[test]
    fn inline_executor_runs_on_dispatch_and_buffers_for_drain() {
        let ex = InlineExecutor::default();
        ex.dispatch(Job {
            buffer_id: BufferId(0),
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            run: Box::new(|| JobResult {
                buffer_id: BufferId(0),
                class: ResultClass::Durability,
                version: 1,
                kind: JobKind::Save,
                merge: Box::new(|e: &mut Editor| e.set_status(crate::status::StatusKind::Info, "merged")),
            }),
        });
        let mut results = ex.drain();
        assert_eq!(results.len(), 1);
        assert!(ex.drain().is_empty(), "drain must consume buffered results");
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        if let JobOutcome::Done(r) = results.remove(0) {
            (r.merge)(&mut e);
        }
        assert_eq!(e.status_text(), "merged");
    }

    #[test]
    fn inline_executor_emits_panicked_outcome() {
        let ex = InlineExecutor::default();
        ex.dispatch(Job {
            buffer_id: BufferId(1), class: ResultClass::BufferLocal, version: 1, kind: JobKind::Save,
            run: Box::new(|| panic!("boom")),
        });
        let out = ex.drain();
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], JobOutcome::Panicked { kind: JobKind::Save, msg, .. } if msg == "boom"));
    }

    // One-shot Save/SwapWrite results are never discarded by is_stale — correctness
    // for an edited-on buffer comes from the version-aware MERGE in save.rs, not here.
    #[test]
    fn one_shot_kinds_are_never_stale() {
        let e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        // Durability: never stale regardless of version or buffer existence.
        let r_save = JobResult { buffer_id: id, class: ResultClass::Durability,
            version: 1, kind: JobKind::Save, merge: Box::new(|_| {}) };
        assert!(!is_stale(&r_save, &e));
        let r_swap = JobResult { buffer_id: id, class: ResultClass::Durability,
            version: 1, kind: JobKind::SwapWrite, merge: Box::new(|_| {}) };
        assert!(!is_stale(&r_swap, &e));
        // Durability for a missing buffer is also never stale.
        let r_missing = JobResult { buffer_id: BufferId(999), class: ResultClass::Durability,
            version: 1, kind: JobKind::Save, merge: Box::new(|_| {}) };
        assert!(!is_stale(&r_missing, &e));
    }

    /// Verify that a job panic does not kill the worker thread.
    ///
    /// Dispatches a job whose closure panics, then a second normal job.  The
    /// second job signals via a channel; if the worker died after the first job
    /// the channel would never be satisfied and the test would time-out/fail.
    ///
    /// Note: the default panic hook prints "panicked at …" to stderr even when
    /// `catch_unwind` catches the panic.  Suppressing it via `set_hook`/
    /// `take_hook` would race with other tests running concurrently in the same
    /// process (hooks are process-global), so we accept the stderr noise.
    #[test]
    fn worker_survives_panicking_job_and_runs_next_job() {
        use std::sync::mpsc;
        let (wake_tx, _wake_rx) = mpsc::channel::<()>();
        let ex = ThreadExecutor::new(wake_tx);

        // First job: panics immediately.
        ex.dispatch(Job {
            buffer_id: BufferId(0),
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            run: Box::new(|| panic!("deliberate test panic — worker must survive")),
        });

        // Second job: signals completion and returns a result.
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        ex.dispatch(Job {
            buffer_id: BufferId(0),
            class: ResultClass::Durability,
            version: 2,
            kind: JobKind::Save,
            run: Box::new(move || {
                done_tx.send(2).unwrap();
                JobResult {
                    buffer_id: BufferId(0),
                    class: ResultClass::Durability,
                    version: 2,
                    kind: JobKind::Save,
                    merge: Box::new(|_| {}),
                }
            }),
        });

        // Block until the second job's closure fires — proves the worker survived.
        assert_eq!(done_rx.recv().unwrap(), 2, "worker must survive a panicking job");

        // Both outcomes arrive on the channel — the panicked job1 as Panicked and the
        // successful job2 as Done. `done_rx.recv()` above unblocks from INSIDE job2's run,
        // which fires BEFORE the worker constructs+sends Done(v2) — so a single drain may
        // hold only [Panicked(v1)]. Accumulate across drains until the awaited Done arrives
        // (no arrival-ordering assumption — keeps this thread test flake-free).
        let mut outcomes = Vec::new();
        while !outcomes.iter().any(|o| matches!(o, JobOutcome::Done(r) if r.version == 2)) {
            outcomes.extend(ex.drain());
        }
    }

    #[test]
    fn coalescible_kind_is_stale_when_version_moved() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().document.version = 2;
        // version 1 result against a buffer at version 2 → stale
        let r_old = JobResult { buffer_id: id, class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::CoalesceProbe, merge: Box::new(|_| {}) };
        assert!(is_stale(&r_old, &e));
        // version 2 result matches → not stale
        let r_cur = JobResult { buffer_id: id, class: ResultClass::BufferLocal,
            version: 2, kind: JobKind::CoalesceProbe, merge: Box::new(|_| {}) };
        assert!(!is_stale(&r_cur, &e));
        // BufferLocal for a missing buffer → always stale
        let r_gone = JobResult { buffer_id: BufferId(999), class: ResultClass::BufferLocal,
            version: 2, kind: JobKind::CoalesceProbe, merge: Box::new(|_| {}) };
        assert!(is_stale(&r_gone, &e));
    }
}
