//! Foreground recovery ownership and exact asynchronous request routing.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use crate::editor::{BufferId, Editor};
use crate::jobs::{Job, JobKind, JobResult, ResultClass};
use crate::recovery_store::{self, CheckpointAck, CheckpointRecord, RecoverySlot, TaggedPath};
use crate::registry::Ctx;

/// Editor-local checked identity, retained by normal and panicked worker outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecoveryRequestId(u64);
#[cfg(test)]
impl RecoveryRequestId { pub(crate) fn for_test(value: u64) -> Self { Self(value) } }

#[derive(Debug, Default)]
pub(crate) struct RecoveryState {
    next_request: u64,
    imports: import::Imports,
    pub(crate) scans: discovery::Scans,
    requests: HashMap<RecoveryRequestId, Request>,
    associations: VecDeque<(BufferId, RecoverySlot)>,
    root: Option<PathBuf>,
}
#[derive(Debug)]
struct Request {
    buffer_id: BufferId,
    slot: RecoverySlot,
    generation: u64,
    version: u64,
    path: Option<PathBuf>,
    started: u64,
    association: bool,
    cancelled: bool,
    detached: bool,
}
impl RecoveryState {
    /// Explicit shared roots are for integration tests; default constructors perform no IO.
    #[cfg(test)]
    pub(crate) fn set_root(&mut self, root: PathBuf) { self.root = Some(root); }
    fn root(&mut self) -> std::io::Result<PathBuf> {
        if let Some(root) = &self.root { return Ok(root.clone()); }
        #[cfg(test)]
        let root = crate::test_support::scratch_path("recovery-state");
        #[cfg(not(test))]
        let root = crate::swap::state_path()?;
        self.root = Some(root.clone());
        Ok(root)
    }
    fn reserve(&mut self) -> Option<RecoveryRequestId> {
        self.next_request = self.next_request.checked_add(1)?;
        Some(RecoveryRequestId(self.next_request))
    }
}

/// Whether a checkpoint entered the worker queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DispatchOutcome { Accepted, Skipped, Rejected }

/// Cancel only this editing instance; queued work retains its slot until completion.
pub(crate) fn cancel_buffer(editor: &mut Editor, id: BufferId) {
    import::cancel_buffer(editor, id);
    let Some(slot) = editor.by_id(id).map(|b| b.recovery_slot.clone()) else { return; };
    for request in editor.recovery.requests.values_mut() {
        if request.buffer_id == id && request.slot.same_instance(&slot) { request.cancelled = true; }
    }
    editor.recovery.associations.retain(|(bid, s)| *bid != id || !s.same_instance(&slot));
}

/// Accepted Save As changes metadata using the current content at the next safe boundary.
pub(crate) fn association_changed(editor: &mut Editor, id: BufferId) {
    let Some(b) = editor.by_id_mut(id) else { return; };
    b.swapped_version = None;
    if !b.document.dirty() { return; }
    let slot = b.recovery_slot.clone();
    if !editor.recovery.associations.iter().any(|(bid, s)| *bid == id && s.same_instance(&slot)) {
        editor.recovery.associations.push_back((id, slot));
    }
}

/// Dispatch current association intents after a merge, before quit re-drive.
pub(crate) fn after_job(ctx: &mut Ctx) {
    import::drive(ctx);
    drive_associations(ctx);
    arm_retries(ctx.editor, ctx.clock.now_ms());
    crate::quit::wake_if_waiting(ctx.editor);
}
/// Run unconditionally after callbacks; does not install buffers or emit Open events.
pub(crate) fn after_callbacks(ctx: &mut Ctx) {
    import::cancel_preparations_for_quit(ctx.editor);
    drive_associations(ctx);
    discovery::drive(ctx);
    arm_retries(ctx.editor, ctx.clock.now_ms());
}
fn drive_associations(ctx: &mut Ctx) {
    let mut remaining = VecDeque::new();
    while let Some((id, slot)) = ctx.editor.recovery.associations.pop_front() {
        let Some(b) = ctx.editor.by_id(id) else { continue; };
        if !b.recovery_slot.same_instance(&slot) || !b.document.dirty() { continue; }
        if b.recovery_request.is_some() { remaining.push_back((id, slot)); continue; }
        dispatch(ctx, id, true);
    }
    ctx.editor.recovery.associations = remaining;
}

pub(crate) fn dispatch_checkpoint(ctx: &mut Ctx, id: BufferId) -> DispatchOutcome {
    let outcome = dispatch(ctx, id, false);
    arm_retries(ctx.editor, ctx.clock.now_ms());
    outcome
}
fn dispatch(ctx: &mut Ctx, id: BufferId, association: bool) -> DispatchOutcome {
    let Some(b) = ctx.editor.by_id(id) else { return DispatchOutcome::Skipped; };
    if b.recovery_request.is_some() { return DispatchOutcome::Skipped; }
    let slot = b.recovery_slot.clone();
    let generation = match slot.reserve_generation() {
        Ok(g) => g,
        Err(e) => { dispatch_failure(ctx.editor, id, association, &e.to_string()); return DispatchOutcome::Rejected; }
    };
    let snapshot = b.document.buffer.snapshot();
    let version = b.document.version;
    let path = b.document.path.clone();
    let lineage = b.document.id.to_hex();
    let provenance = b.recovery_provenance.clone();
    let source = b.recovery_source.clone();
    let Some(request_id) = ctx.editor.recovery.reserve() else {
        dispatch_failure(ctx.editor, id, association, "recovery request identity exhausted"); return DispatchOutcome::Rejected;
    };
    let root = match ctx.editor.recovery.root() {
        Ok(root) => root,
        Err(e) => { dispatch_failure(ctx.editor, id, association, &e.to_string()); return DispatchOutcome::Rejected; }
    };
    let request = Request { buffer_id: id, slot: slot.clone(), generation, version,
        path: path.clone(), started: ctx.clock.now_ms(), association, cancelled: false, detached: false };
    ctx.editor.by_id_mut(id).expect("captured buffer").recovery_generation = generation;
    ctx.editor.recovery.requests.insert(request_id, request);
    let fs = ctx.fs.clone();
    let kind = JobKind::Recovery(request_id);
    let job = Job { buffer_id: id, class: ResultClass::Durability, version, kind, save_request: None,
        run: Box::new(move || {
            let result = (|| {
                let root = resolve_association(&*fs, &root)?;
                let association = path.as_deref().map(|p| resolve_association(&*fs, p)
                    .map(|p| TaggedPath::from_path(&p))).transpose()?;
                let record = with_predecessor(CheckpointRecord::new(
                    generation, lineage, version, association, provenance), source.as_ref());
                recovery_store::checkpoint(&*fs, &root, &slot, &record, &snapshot.to_string())
            })().map_err(|e: recovery_store::RecoveryError| e.to_string());
            JobResult { buffer_id: id, class: ResultClass::Durability, version, kind,
                merge: Box::new(move |editor| complete(editor, request_id, result)) }
        }) };
    if ctx.executor.try_dispatch(job).is_err() {
        complete(ctx.editor, request_id, Err("recovery worker queue closed".into()));
        return DispatchOutcome::Rejected;
    }
    let b = ctx.editor.by_id_mut(id).expect("dispatch does not remove buffers");
    b.recovery_request = Some(request_id);
    b.swap_in_flight = true;
    DispatchOutcome::Accepted
}

pub(crate) use crate::fsx::canonicalize_with_missing_suffix as resolve_association;
fn complete(editor: &mut Editor, id: RecoveryRequestId, result: Result<CheckpointAck, String>) {
    let Some(request) = editor.recovery.requests.remove(&id) else { return; };
    let Some(b) = editor.by_id_mut(request.buffer_id) else { return; };
    if !b.recovery_slot.same_instance(&request.slot) { return; }
    if b.recovery_request == Some(id) {
        b.recovery_request = None;
        b.swap_in_flight = false;
    }
    if request.cancelled {
        if result.is_err() { b.recovery_retry.failed(); } else { b.recovery_retry.succeeded(); }
        return;
    }
    match result {
        Ok(ack) => {
            b.recovery_retry.succeeded();
            if b.recovery_generation == request.generation && b.document.path == request.path {
                b.recovery_ack = Some(ack);
                b.last_swap_at = Some(request.started);
                b.swapped_version = Some(request.version);
                b.recovery_protection_failure = None;
            }
        }
        Err(_) if request.detached => { b.recovery_retry.failed(); }
        Err(message) => {
            let cancelled_quit = request.association && (editor.quit || editor.quit_drain.is_some());
            if cancelled_quit { crate::quit::cancel(editor); }
            let message = if cancelled_quit { format!("{message} — quit cancelled") } else { message };
            failure(editor, request.buffer_id, &message);
        }
    }
}
fn dispatch_failure(editor: &mut Editor, id: BufferId, association: bool, message: &str) {
    if association && (editor.quit || editor.quit_drain.is_some()) {
        crate::quit::cancel(editor);
        failure(editor, id, &format!("{message} — quit cancelled"));
    } else { failure(editor, id, message); }
}
fn failure(editor: &mut Editor, id: BufferId, message: &str) {
    if let Some(b) = editor.by_id_mut(id) {
        b.recovery_protection_failure = Some(message.into());
        b.recovery_retry.failed();
    }
    editor.set_status_full(crate::status::StatusKind::Error, format!("recovery checkpoint failed: {message}"),
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
}
pub(crate) fn on_panic(editor: &mut Editor, id: RecoveryRequestId, message: &str) {
    if discovery::panic_request(editor, id, message) { return; }
    if import::panic_request(editor, id, message) { return; }
    complete(editor, id, Err(format!("internal error: {message}")));
}

#[cfg(test)]
mod tests;

mod import;
mod progress;
pub(crate) use import::{begin_selected, cancel_batch, imports_busy, protected_sources};

fn with_predecessor(record: CheckpointRecord, source: Option<&crate::recovery_discovery::SelectionToken>) -> CheckpointRecord {
    if let Some(crate::recovery_discovery::SelectionToken::V2 { owner, generation }) = source {
        record.with_predecessor(owner.clone(), *generation)
    } else { record }
}

pub(crate) mod discovery;
pub(crate) use discovery::{bootstrap, opened, review};

mod pending;
pub(crate) use pending::{has_pending_work, pending_deadline, timeout_tick};

mod retry;
pub(crate) use retry::{RetryState, arm_retries};
