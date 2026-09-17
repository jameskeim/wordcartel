//! Concrete worker waits. Detachment revokes foreground exit blockers, never worker IO.
use super::*;

pub(crate) fn has_pending_work(editor: &Editor) -> bool {
    import::blocks_exit(editor) || !editor.recovery.associations.is_empty()
        || editor.recovery.requests.values().any(|r| r.association && !r.detached)
}
fn blocks_association(editor: &Editor, request: &Request) -> bool {
    request.association || editor.recovery.associations.iter()
        .any(|(_, slot)| slot.same_instance(&request.slot))
}
pub(crate) fn pending_deadline(editor: &Editor, _now: u64) -> Option<u64> {
    if !crate::quit::in_progress(editor) { return None; }
    editor.recovery.requests.values()
        .filter(|r| !r.detached && blocks_association(editor, r))
        .map(|r| r.started.saturating_add(5_000))
        .chain(import::deadline(editor)).min()
}
pub(crate) fn timeout_tick(editor: &mut Editor, now: u64) {
    if !crate::quit::in_progress(editor) { return; }
    let expired_requests: Vec<_> = editor.recovery.requests.iter()
        .filter(|(_, r)| !r.detached && blocks_association(editor, r)
            && now >= r.started.saturating_add(5_000))
        .map(|(id, _)| *id).collect();
    let mut expired = import::expire(editor, now);
    let mut expired_slots = import::detached_slots(editor);
    for id in expired_requests {
        let request = editor.recovery.requests.get_mut(&id).expect("captured request");
        request.detached = true;
        expired_slots.push(request.slot.clone());
        expired = true;
    }
    if !expired { return; }
    editor.recovery.associations.retain(|(_, slot)|
        !expired_slots.iter().any(|s| s.same_instance(slot)));
    crate::quit::cancel(editor);
    editor.set_status_full(crate::status::StatusKind::Warning,
        "Recovery IO is still pending; quit cancelled", crate::status::StatusLifetime::Sticky,
        crate::status::StatusSource::Host, None);
}
#[cfg(test)]
#[path = "pending/tests.rs"]
mod regression_tests;
