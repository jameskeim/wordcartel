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
    #[cfg(test)]
    CoalesceProbe, // test-only stand-in for a future coalescible kind
}

/// A unit of background work, dispatched for a document version, run on a
/// worker, merged back on the foreground.
pub struct Job {
    pub version: u64,
    pub kind: JobKind,
    /// Runs on the worker thread; must not touch the Editor directly.
    pub run: Box<dyn FnOnce() -> JobResult + Send>,
}

/// What a worker hands back: its own foreground merge effect.
pub struct JobResult {
    pub version: u64,
    pub kind: JobKind,
    /// Applied on the foreground before the next draw. By contract this touches
    /// only non-document bookkeeping; any document-text change must route
    /// through `editor.apply`.
    pub merge: Box<dyn FnOnce(&mut Editor) + Send>,
}

/// The single staleness predicate (spec §4.1 staleness policy).
pub fn is_stale(
    kind: JobKind,
    #[allow(unused_variables)] result_version: u64,
    #[allow(unused_variables)] current_version: u64,
) -> bool {
    match kind {
        JobKind::Save | JobKind::SwapWrite => false, // one-shot: always applies
        #[cfg(test)]
        JobKind::CoalesceProbe => result_version != current_version,
    }
}

pub trait Executor {
    /// Enqueue a job for the worker.
    fn dispatch(&self, job: Job);
    /// Non-blocking: collect any results ready now (consumes them).
    fn drain(&self) -> Vec<JobResult>;
}

/// Deterministic test executor: runs `job.run()` immediately on `dispatch`,
/// buffers the result for `drain`. No threads, no flake.
#[derive(Default)]
pub struct InlineExecutor {
    pending: RefCell<Vec<JobResult>>,
}

impl Executor for InlineExecutor {
    fn dispatch(&self, job: Job) {
        let result = (job.run)();
        self.pending.borrow_mut().push(result);
    }
    fn drain(&self) -> Vec<JobResult> {
        self.pending.borrow_mut().drain(..).collect()
    }
}

/// Production executor: one worker thread, FIFO. The worker pushes each
/// JobResult onto an internal channel (drained by `drain`) and sends a unit
/// "wake" nudge on `wake` after each result so the main loop can wake and drain.
pub struct ThreadExecutor {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<JobResult>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadExecutor {
    pub fn new(wake: Sender<()>) -> ThreadExecutor {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<JobResult>();
        let worker = std::thread::Builder::new()
            .name("wcartel-jobs".into())
            .spawn(move || {
                // FIFO: process jobs in dispatch order. Exit when job_tx drops.
                while let Ok(job) = job_rx.recv() {
                    let result = (job.run)();
                    if result_tx.send(result).is_err() { break; }
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
    fn drain(&self) -> Vec<JobResult> {
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
    use crate::editor::Editor;

    #[test]
    fn thread_executor_runs_job_on_worker_and_drains_result() {
        use std::sync::mpsc;
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let (wake_tx, _wake_rx) = mpsc::channel::<()>();
        let ex = ThreadExecutor::new(wake_tx);
        ex.dispatch(Job {
            version: 7,
            kind: JobKind::Save,
            run: Box::new(move || {
                done_tx.send(7).unwrap();
                JobResult { version: 7, kind: JobKind::Save,
                    merge: Box::new(|e: &mut crate::editor::Editor| e.status = "worker".into()) }
            }),
        });
        assert_eq!(done_rx.recv().unwrap(), 7, "worker must run the job");
        let mut results = Vec::new();
        while results.is_empty() { results = ex.drain(); }
        assert_eq!(results[0].version, 7);
    }

    #[test]
    fn inline_executor_runs_on_dispatch_and_buffers_for_drain() {
        let ex = InlineExecutor::default();
        ex.dispatch(Job {
            version: 1,
            kind: JobKind::Save,
            run: Box::new(|| JobResult {
                version: 1,
                kind: JobKind::Save,
                merge: Box::new(|e: &mut Editor| e.status = "merged".into()),
            }),
        });
        let mut results = ex.drain();
        assert_eq!(results.len(), 1);
        assert!(ex.drain().is_empty(), "drain must consume buffered results");
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        (results.remove(0).merge)(&mut e);
        assert_eq!(e.status, "merged");
    }

    // One-shot Save/SwapWrite results are never discarded by is_stale — correctness
    // for an edited-on buffer comes from the version-aware MERGE in save.rs, not here.
    #[test]
    fn one_shot_kinds_are_never_stale() {
        assert!(!is_stale(JobKind::Save, 1, 99));
        assert!(!is_stale(JobKind::SwapWrite, 1, 99));
    }

    #[test]
    fn coalescible_kind_is_stale_when_version_moved() {
        assert!(is_stale(JobKind::CoalesceProbe, 1, 2));
        assert!(!is_stale(JobKind::CoalesceProbe, 2, 2));
    }
}
