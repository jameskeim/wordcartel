//! Edge-triggered worker scans and deferred, epoch-checked offers.
use super::*;
use std::collections::HashSet;
use crate::recovery_discovery::{Candidate, CandidateTime, ScanScope, SelectionToken};

#[derive(Debug, PartialEq, Eq, Hash)]
struct UnavailableIdentity { path: PathBuf, time: CandidateTime, reason: String }
fn unavailable_identity(row: &Candidate) -> Option<UnavailableIdentity> {
    Some(UnavailableIdentity { path: row.source_path.clone(), time: row.timestamp.clone(),
        reason: row.unavailable.as_ref()?.clone() })
}
use crate::recovery_picker::RecoveryPicker;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Intent {
    origin: Option<(BufferId, PathBuf)>,
    manual: bool,
}
#[derive(Debug)]
struct ScanRequest { epoch: u64, intent: Intent }
#[derive(Debug, Default)]
pub(crate) struct Scans {
    epoch: u64,
    bootstrapped: bool,
    queue: VecDeque<Intent>,
    requests: HashMap<RecoveryRequestId, ScanRequest>,
    offers: VecDeque<(Intent, Result<Vec<Candidate>, String>)>,
    pub(crate) dismissed: HashSet<SelectionToken>,
    pub(crate) opened: HashSet<SelectionToken>,
    dismissed_unavailable: HashSet<UnavailableIdentity>,
    reported_errors: HashSet<String>,
}
/// Schedule the one all-candidate startup scan before the first blocking receive.
pub(crate) fn bootstrap(ctx: &mut Ctx) {
    if ctx.editor.recovery.scans.bootstrapped { return; }
    ctx.editor.recovery.scans.bootstrapped = true;
    ctx.editor.recovery.scans.queue.push_front(Intent { origin: None, manual: false });
    drive(ctx);
}
/// Queue an assessment after successful disk-buffer installation. No filesystem work here.
pub(crate) fn opened(editor: &mut Editor, id: BufferId, path: &Path) {
    let intent = Intent { origin: Some((id,path.to_owned())), manual: false };
    let scans = &mut editor.recovery.scans;
    // A running scan may already have read its snapshot; retain one later assessment.
    if !scans.queue.contains(&intent) {
        scans.queue.push_back(intent);
    }
}
/// The shared registry route, including palette, menu and plugin invocations.
pub(crate) fn review(ctx: &mut Ctx) {
    if ctx.editor.quit || ctx.editor.quit_drain.is_some() || ctx.editor.prompt.is_some()
        || ctx.editor.file_browser.is_some() || ctx.editor.pending_after_save.is_some()
        || ctx.editor.pending_save_as.is_some() || super::imports_busy(ctx.editor) {
        status(ctx.editor,"Recovery review is busy; finish the current save, close or recovery first"); return;
    }
    crate::overlays::close_all(ctx.editor);
    cancel_scans(ctx.editor);
    ctx.editor.recovery_picker = Some(RecoveryPicker::loading());
    ctx.editor.recovery.scans.queue.push_front(Intent { origin: None, manual: true });
    drive(ctx);
}
/// Cancel this picker's manual scan without losing other buffers' assessments.
pub(crate) fn cancel_scans(editor: &mut Editor) {
    let scans = &mut editor.recovery.scans;
    scans.queue.retain(|intent| !intent.manual);
    scans.offers.retain(|(intent, _)| !intent.manual);
    scans.requests.retain(|_, request| !request.intent.manual);
}
pub(crate) fn dismiss(editor: &mut Editor, row: &Candidate) {
    if let Some(token) = &row.token {
        editor.recovery.scans.dismissed.insert(token.clone());
    } else if let Some(identity) = unavailable_identity(row) {
        editor.recovery.scans.dismissed_unavailable.insert(identity);
    }
}
fn status(editor: &mut Editor, message: &str) {
    editor.set_status_full(crate::status::StatusKind::Error, message,
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
}
pub(super) fn drive(ctx: &mut Ctx) {
    crate::recovery_picker::batch_boundary(ctx.editor);
    offer(ctx.editor);
    if ctx.editor.quit || ctx.editor.quit_drain.is_some() { return; }
    if ctx.editor.recovery.scans.requests.values().any(|r| r.epoch == ctx.editor.recovery.scans.epoch) { return; }
    let Some(intent) = ctx.editor.recovery.scans.queue.pop_front() else { return; };
    let Some(id) = ctx.editor.recovery.reserve() else {
        ctx.editor.recovery.scans.offers.push_back((intent,Err("recovery request identity exhausted".into())));
        offer(ctx.editor); return;
    };
    let root = match ctx.editor.recovery.root() { Ok(r) => r, Err(e) => {
        ctx.editor.recovery.scans.offers.push_back((intent,Err(e.to_string()))); offer(ctx.editor); return;
    } };
    let epoch = ctx.editor.recovery.scans.epoch;
    let scope = intent.origin.as_ref().map_or(ScanScope::All,|(_,p)| ScanScope::Associated(p.clone()));
    ctx.editor.recovery.scans.requests.insert(id,ScanRequest {epoch,intent});
    let fs = ctx.fs.clone();
    let bid = ctx.editor.active().id;
    let kind = JobKind::Recovery(id);
    let job = Job {buffer_id:bid,version:0,class:ResultClass::Durability,kind,save_request:None,
        run:Box::new(move || {
            let result = crate::recovery_discovery::scan(&*fs,&root,&scope).map_err(|e| e.to_string());
            JobResult {buffer_id:bid,version:0,class:ResultClass::Durability,kind,
                merge:Box::new(move |editor| complete(editor,id,result)) }
        }) };
    if ctx.executor.try_dispatch(job).is_err() { complete(ctx.editor,id,Err("recovery worker queue closed".into())); }
    offer(ctx.editor);
}
fn complete(editor: &mut Editor, id: RecoveryRequestId, result: Result<Vec<Candidate>,String>) {
    let Some(request) = editor.recovery.scans.requests.remove(&id) else { return; };
    if request.epoch != editor.recovery.scans.epoch { return; }
    if request.intent.manual {
        editor.recovery.scans.offers.push_front((request.intent, result));
    } else {
        editor.recovery.scans.offers.push_back((request.intent, result));
    }
}
pub(super) fn panic_request(editor: &mut Editor, id: RecoveryRequestId, message: &str) -> bool {
    if !editor.recovery.scans.requests.contains_key(&id) { return false; }
    complete(editor,id,Err(format!("Recovery scan failed: {message}"))); true
}
fn safe(editor: &Editor) -> bool {
    !editor.quit && editor.quit_drain.is_none() && editor.pending_mark.is_none()
        && editor.pending_after_save.is_none() && editor.pending_save_as.is_none()
        && !crate::overlays::OverlayId::ALL.iter().any(|id|
            *id != crate::overlays::OverlayId::Splash && (id.row().is_active)(editor))
        && !super::imports_busy(editor)
}
fn offer(editor: &mut Editor) {
    if editor.quit || editor.quit_drain.is_some() { return; }
    while let Some((intent,result)) = editor.recovery.scans.offers.pop_front() {
        if intent.manual {
            if editor.recovery_picker.as_ref().is_some_and(RecoveryPicker::is_loading) {
                editor.recovery_picker = Some(match result {
                    Ok(rows) => RecoveryPicker::ready(rows), Err(e) => RecoveryPicker::error(e),
                });
            }
            continue;
        }
        if let Some((id,path)) = &intent.origin {
            if editor.active().id != *id
                || editor.by_id(*id).is_none_or(|b| b.document.path.as_ref() != Some(path)) {
                continue;
            }
        }
        if result.as_ref().err().is_some_and(|error| editor.recovery.scans.reported_errors.contains(error)) {
            continue;
        }
        let result = result.map(|rows| rows.into_iter().filter(|row| !row.busy
            && !unavailable_identity(row).is_some_and(|key| editor.recovery.scans.dismissed_unavailable.contains(&key))
            && row.token.as_ref().is_none_or(|token| {
            !editor.recovery.scans.dismissed.contains(token) && !editor.recovery.scans.opened.contains(token)
                && !editor.buffers.iter().any(|b| b.recovery_source.as_ref() == Some(token))
                && !super::import::pending_token(editor,token)
        })).collect::<Vec<_>>());
        if result.as_ref().is_ok_and(Vec::is_empty) { continue; }
        if !safe(editor) { editor.recovery.scans.offers.push_front((intent,result)); break; }
        if let Err(error) = &result { editor.recovery.scans.reported_errors.insert(error.clone()); }
        editor.splash = None;
        editor.recovery_picker = Some(match result {
                    Ok(rows) => RecoveryPicker::ready(rows), Err(e) => RecoveryPicker::error(e),
                });
    }
}
#[cfg(test)]
#[path = "discovery/tests.rs"]
mod regression_tests;
