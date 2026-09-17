use super::*;
use crate::recovery_discovery::{Candidate, PrepareOutcome, PreparedRecovery};
use super::progress::Progress;
use std::sync::Arc;
#[derive(Default)]
pub(super) struct Imports {
    queue: VecDeque<Candidate>,
    requests: HashMap<RecoveryRequestId, ImportRequest>,
    ready: VecDeque<(RecoveryRequestId, PreparedRecovery, Option<PathBuf>)>,
}
impl std::fmt::Debug for Imports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Imports").field("pending", &self.requests.len()).finish()
    }
}
struct ImportRequest {
    source_path: PathBuf,
    token: Option<crate::recovery_discovery::SelectionToken>,
    buffer: Option<(BufferId, RecoverySlot)>,
    progress: Arc<Progress>,
    cancelled: bool,
    detached: bool,
    started: u64,
    version: u64,
    path: Option<PathBuf>,
}
pub(crate) fn begin_selected(ctx: &mut Ctx, selected: Vec<Candidate>) {
    ctx.editor.recovery.imports.queue.extend(selected);
    drive(ctx);
}
pub(crate) fn cancel_batch(editor: &mut Editor) {
    editor.recovery.imports.queue.clear();
    for request in editor.recovery.imports.requests.values_mut() {
        request.cancelled = true;
        request.progress.cancel();
    }
}
pub(crate) fn imports_busy(editor: &Editor) -> bool { !editor.recovery.imports.requests.is_empty() }
pub(super) fn blocks_exit(editor: &Editor) -> bool {
    editor.recovery.imports.requests.values().any(|r| r.buffer.is_some() && !r.detached)
}
pub(super) fn detached_slots(editor: &Editor) -> Vec<RecoverySlot> {
    editor.recovery.imports.requests.values().filter(|r| r.detached)
        .filter_map(|r| r.buffer.as_ref().map(|(_,s)| s.clone())).collect()
}
pub(super) fn deadline(editor: &Editor) -> Option<u64> {
    editor.recovery.imports.requests.values().filter(|r| !r.detached && r.buffer.is_some())
        .map(|r| r.started.saturating_add(5_000)).min()
}
pub(super) fn expire(editor: &mut Editor, now: u64) -> bool {
    let mut expired = false;
    for r in editor.recovery.imports.requests.values_mut().filter(|r|
        !r.detached && r.buffer.is_some() && now >= r.started.saturating_add(5_000)) {
        r.detached = true;
        r.progress.cancel();
        expired = true;
    }
    if expired { cancel_preparations_for_quit(editor); }
    expired
}
pub(super) fn cancel_buffer(editor: &mut Editor, id: BufferId) {
    if editor.recovery.imports.requests.values()
        .any(|r| r.buffer.as_ref().is_some_and(|(bid, _)| *bid == id)) {
        cancel_batch(editor);
    }
}
pub(super) fn panic_request(editor: &mut Editor, id: RecoveryRequestId, message: &str) -> bool {
    if !editor.recovery.imports.requests.contains_key(&id) { return false; }
    finish(editor, id, Err(format!("internal error: {message}"))); true
}
pub(super) fn cancel_preparations_for_quit(editor: &mut Editor) {
    if crate::quit::in_progress(editor) {
        editor.recovery.imports.queue.clear();
        for request in editor.recovery.imports.requests.values_mut().filter(|r| r.buffer.is_none()) {
            request.cancelled = true; request.progress.cancel();
        }
    }
}
pub(super) fn drive(ctx: &mut Ctx) {
    cancel_preparations_for_quit(ctx.editor);
    while let Some((id, prepared, dir)) = ctx.editor.recovery.imports.ready.pop_front() {
        if ctx.editor.recovery.imports.requests.get(&id).is_some_and(|r| !r.cancelled) {
            install(ctx, id, prepared, dir);
        } else { ctx.editor.recovery.imports.requests.remove(&id); }
    }
    while ctx.editor.recovery.imports.requests.is_empty() {
        let Some(candidate) = ctx.editor.recovery.imports.queue.pop_front() else { break; };
        if focus_protected(ctx.editor, &candidate) { continue; }
        let Some(id) = ctx.editor.recovery.reserve() else {
            status(ctx.editor, "recovery request identity exhausted"); break;
        };
        let root = match ctx.editor.recovery.root() {
            Ok(r) => r,
            Err(e) => { status(ctx.editor, &e.to_string()); continue; }
        };
        ctx.editor.recovery.imports.requests.insert(id, ImportRequest {
            source_path: candidate.source_path.clone(), token: candidate.token.clone(), buffer: None,
            progress: Arc::new(Progress::default()), cancelled: false, detached: false,
            started: ctx.clock.now_ms(), version: 0, path: None,
        });
        let fs = ctx.fs.clone(); let bid = ctx.editor.active().id; let kind = JobKind::Recovery(id);
        let job = Job { buffer_id: bid, version: 0, class: ResultClass::Durability, kind, save_request: None,
            run: Box::new(move || {
                let result = crate::recovery_discovery::prepare(&*fs, &root, &candidate).map(|prepared| {
                    let dir = match &prepared {
                        PrepareOutcome::Ready(p) => p.candidate.association.as_ref()
                            .or(p.candidate.provenance.as_ref())
                        .and_then(TaggedPath::local_path).and_then(|p| p.parent().map(Path::to_owned))
                        .filter(|p| fs.list_dir(p, Some(1)).is_ok()), _ => None };
                    (prepared, dir)
                }).map_err(|e| e.to_string());
                JobResult { buffer_id: bid, version: 0, class: ResultClass::Durability, kind,
                    merge: Box::new(move |editor| match result {
                        Ok((PrepareOutcome::Ready(p), dir)) => editor.recovery.imports.ready.push_back((id,p,dir)),
                        Ok((PrepareOutcome::Changed(row),_)) => {
                            let token=editor.recovery.imports.requests.get(&id).and_then(|r| r.token.clone());
                            crate::recovery_picker::row_result(editor, token.as_ref(), Some(row),
                                "Recovery copy changed; review and select it again");
                            finish(editor,id,Err("recovery copy changed; review and select it again".into()));
                        },
                        Err(e) => finish(editor,id,Err(e)),
                    }) }
            }) };
        if ctx.executor.try_dispatch(job).is_err() { finish(ctx.editor,id,Err("recovery worker queue closed".into())); }
    }
}
fn protected(buffer: &crate::editor::Buffer) -> bool {
    !buffer.document.dirty()
        || (buffer.swapped_version == Some(buffer.document.version) && buffer.recovery_ack.is_some())
}
/// A successful handoff can retire the source. Focusing its live protected document
/// uses only the exact selection token; it confers no new deletion authority.
fn focus_protected(editor: &mut Editor, candidate: &Candidate) -> bool {
    let Some(token) = candidate.token.as_ref() else { return false; };
    let Some(index) = editor.buffers.iter()
        .position(|b| b.recovery_source.as_ref() == Some(token) && protected(b))
        else { return false; };
    editor.switch_to_index(index);
    crate::derive::rebuild(editor); crate::nav::ensure_visible(editor);
    true
}
fn install(ctx: &mut Ctx, request: RecoveryRequestId, prepared: PreparedRecovery, dir: Option<PathBuf>) {
    let (candidate, body, lease) = prepared.into_parts();
    let existing = ctx.editor.buffers.iter()
        .position(|b| b.recovery_source.is_some() && b.recovery_source == candidate.token);
    let retry = existing.is_some();
    let index = if let Some(index) = existing { index } else {
        let id = ctx.editor.alloc_id();
        let mut b = crate::editor::Buffer::from_text(id, &body, None, ctx.editor.active().view.area);
        b.document.saved_version = None;
        b.recovery_provenance = candidate.association.clone().or(candidate.provenance.clone());
        b.recovery_save_dir = dir;
        b.recovery_source = candidate.token.clone();
        b.recovery_source_path = Some(candidate.source_path.clone());
        ctx.editor.buffers.push(b); ctx.editor.buffers.len()-1
    };
    ctx.editor.switch_to_index(index);
    if let Some(token) = &candidate.token { ctx.editor.recovery.scans.opened.insert(token.clone()); }
    crate::derive::rebuild(ctx.editor); crate::nav::ensure_visible(ctx.editor);
    let b = ctx.editor.active();
    let bid = b.id;
    let slot = b.recovery_slot.clone();
    // Capture BEFORE the recovered Open event can enter the callback pump.
    let snapshot = b.document.buffer.snapshot();
    let version = b.document.version;
    let lineage = b.document.id.to_hex();
    let provenance = b.recovery_provenance.clone();
    let path = b.document.path.clone();
    if b.recovery_request.is_some() || (retry && protected(b)) {
        ctx.editor.recovery.imports.requests.remove(&request);
        return;
    }
    let r = ctx.editor.recovery.imports.requests.get_mut(&request).expect("live preparation");
    r.buffer = Some((bid,slot.clone())); r.version = version; r.path = path.clone(); r.started = ctx.clock.now_ms();
    // A dispatched prepare and handoff are distinct requests even though ownership transfers.
    let Some(handoff_id) = ctx.editor.recovery.reserve() else {
        install_failed(ctx.editor,request,retry,"recovery request identity exhausted".into()); return;
    };
    let r = ctx.editor.recovery.imports.requests.remove(&request).expect("transferring preparation");
    let progress = r.progress.clone();
    ctx.editor.recovery.imports.requests.insert(handoff_id,r);
    let request = handoff_id;
    let generation = match slot.reserve_generation() {
        Ok(g) => g,
        Err(e) => { install_failed(ctx.editor, request, retry, e.to_string()); return; }
    };
    let root = match ctx.editor.recovery.root() {
        Ok(r) => r,
        Err(e) => { install_failed(ctx.editor, request, retry, e.to_string()); return; }
    };
    let b = ctx.editor.active_mut();
    b.recovery_generation = generation;
    b.recovery_request = Some(request);
    b.swap_in_flight = true;
    let fs = ctx.fs.clone(); let kind = JobKind::Recovery(request);
    let job = Job {buffer_id:bid, version, class:ResultClass::Durability, kind, save_request:None,
        run:Box::new(move || {
            // Keep the exact source lease until retirement completes or unwinds.
            let _lease = lease;
            let result = (|| {
                let root = resolve_association(&*fs,&root)?;
                let association = path.as_deref().map(|p| resolve_association(&*fs, p)
                    .map(|p| TaggedPath::from_path(&p))).transpose()?;
                let record = with_predecessor(CheckpointRecord::new(
                    generation, lineage, version, association, provenance), candidate.token.as_ref());
                let ack = recovery_store::checkpoint(&*fs,&root,&slot,&record,&snapshot.to_string())?;
                progress.record_ack(ack.clone());
                if progress.authorize() && !retry
                    && matches!(candidate.token, Some(crate::recovery_discovery::SelectionToken::V2 { .. })) {
                    fs.remove_file(&candidate.source_path)?;
                    fs.sync_dir_strict(candidate.source_path.parent().expect("validated source parent"))?;
                }
                progress.finish(); Ok::<_, recovery_store::RecoveryError>(ack)
            })().map_err(|e| e.to_string());
            JobResult {buffer_id:bid,version,class:ResultClass::Durability,kind,
                merge:Box::new(move |editor| finish(editor,request,result)) }
        }) };
    if ctx.executor.try_dispatch(job).is_err() {
        finish(ctx.editor, request, Err("recovery worker queue closed".into()));
    }
    if !retry { crate::plugin::fire_event(ctx.editor, crate::plugin::PluginEventKind::Open,None); }
}
fn install_failed(editor: &mut Editor, request: RecoveryRequestId, retry: bool, message: String) {
    finish(editor,request,Err(message));
    if !retry { crate::plugin::fire_event(editor,crate::plugin::PluginEventKind::Open,None); }
}
fn status(editor: &mut Editor, message: &str) {
    editor.set_status_full(crate::status::StatusKind::Error, message.to_owned(),
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
}
fn finish(editor: &mut Editor, id: RecoveryRequestId, result: Result<CheckpointAck, String>) {
    let Some(request) = editor.recovery.imports.requests.remove(&id) else { return; };
    let handoff = request.buffer.is_some();
    if let Some((bid,slot)) = request.buffer {
        if let Some(b) = editor.by_id_mut(bid).filter(|b| b.recovery_slot.same_instance(&slot)) {
            if b.recovery_request == Some(id) { b.recovery_request = None; b.swap_in_flight = false; }
            if result.is_ok() || request.progress.ack().is_some() {
                b.recovery_retry.succeeded();
            } else {
                b.recovery_retry.failed();
            }
            // Cancellation revokes retirement, not evidence of an already durable snapshot.
            let ack = result.as_ref().ok().or_else(|| request.progress.ack());
            if let Some(ack) = ack.filter(|ack|
                b.document.path == request.path && b.recovery_generation == ack.generation()) {
                b.recovery_ack = Some(ack.clone()); b.swapped_version = Some(request.version);
                b.last_swap_at = Some(request.started); b.recovery_protection_failure = None;
            } else if !request.cancelled {
                if let Err(e) = &result {
                    if !request.progress.protected() {
                        b.recovery_protection_failure = Some(e.clone());
                    }
                }
            }
        }
    }
    if request.cancelled || request.detached { return; }
    if let Err(message) = result {
        crate::recovery_picker::row_result(editor,request.token.as_ref(),None,&message);
        let message = if request.progress.protected() {
            format!("recovered copy protected; source cleanup uncertain: {message}")
        } else { format!("recovery failed before source cleanup: {message}") };
        if handoff && (editor.quit || editor.quit_drain.is_some()) {
            crate::quit::cancel(editor);
            status(editor, &format!("{message} — quit cancelled"));
        }
        else { status(editor,&message); }
    }
}
#[cfg(test)]
#[path = "import/tests.rs"]
mod regression_tests;

/// Pure carrier protection metadata, including preparation bodies held only by the worker.
pub(crate) fn protected_sources(editor: &Editor) -> Vec<PathBuf> {
    editor.recovery.imports.queue.iter().map(|c| c.source_path.clone())
        .chain(editor.recovery.imports.requests.values().map(|r| r.source_path.clone()))
        .chain(editor.buffers.iter().filter_map(|b| b.recovery_source_path.clone().or_else(|| match &b.recovery_source {
            Some(crate::recovery_discovery::SelectionToken::Legacy {path,..}) => Some(path.clone()), _ => None,
        }))).collect()
}
pub(super) fn pending_token(editor: &Editor, token: &crate::recovery_discovery::SelectionToken) -> bool {
    editor.recovery.imports.queue.iter().any(|c| c.token.as_ref() == Some(token))
        || editor.recovery.imports.requests.values().any(|r| r.token.as_ref() == Some(token))
}
