//! Session-wide save/review/quit decisions. Discard applies to the version reviewed;
//! an empty queue is rechecked against live buffers before it can authorize exit.

use crate::editor::{BufferId, Editor, PostSaveAction, QuitDrain, QuitMode};
use crate::registry::Ctx;

fn busy(editor: &mut Editor) -> bool {
    if !editor.quit && editor.pending_after_save.is_none() && editor.pending_save_as.is_none()
        && editor.quit_drain.is_none() { return false; }
    editor.set_status_full(crate::status::StatusKind::Warning,
        "another save or quit is in progress — try again",
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
    true
}

/// Begin an explicit Save and Quit: preserve the active document's Save/Save As step,
/// then drain every other dirty ordinary document, including edits made while waiting.
pub(crate) fn save_and_quit(ctx: &mut Ctx) {
    if busy(ctx.editor) { return; }
    let id = ctx.editor.active().id;
    ctx.editor.quit_drain = Some(QuitDrain::new([id].into(), QuitMode::SaveAll));
    crate::save::dispatch_save_then(ctx, PostSaveAction::ContinueQuitDrain);
}

/// Start a Save All / Review Each quit selected from the summary prompt.
pub(crate) fn start(ctx: &mut Ctx, mode: QuitMode) {
    // A confirmed summary choice replaces an earlier quit attempt. In particular,
    // Command::Quit may have closed that attempt's filename picker to show this summary.
    if in_progress(ctx.editor) {
        if ctx.editor.pending_save_as == Some(PostSaveAction::ContinueQuitDrain) {
            ctx.editor.pending_save_as = None;
        }
        cancel(ctx.editor);
    }
    if busy(ctx.editor) { return; }
    ctx.editor.quit_drain = Some(QuitDrain::new(Default::default(), mode));
    drive(ctx);
}

fn drive(ctx: &mut Ctx) {
    crate::jobs_apply::drive_quit_drain(ctx.editor, ctx.executor, ctx.clock, &ctx.msg_tx, &ctx.fs);
}

/// Refill only an exhausted queue. Returning None proves that no live dirty ordinary
/// buffer remains except versions the user explicitly chose to discard in this flow.
pub(crate) fn refill(editor: &mut Editor) -> Option<BufferId> {
    let drain = editor.quit_drain.as_ref()?;
    let queue = editor.buffers.iter()
        .filter(|b| editor.is_dirty(b.id)
            && drain.discarded_versions.get(&b.id) != Some(&b.document.version))
        .map(|b| b.id).collect();
    let drain = editor.quit_drain.as_mut().expect("quit flow still present");
    drain.queue = queue;
    drain.queue.front().copied()
}

/// Bind a review prompt to the identity and version it actually displayed.
pub(crate) fn show_review(editor: &mut Editor, id: BufferId) {
    let version = editor.by_id(id).expect("driver selected a live buffer").document.version;
    if let Some(drain) = editor.quit_drain.as_mut() { drain.reviewing = Some((id, version)); }
    let name = crate::workspace::buffer_display_name(editor, id);
    editor.open_prompt(crate::prompt::Prompt::quit_review_buffer(&name));
}

/// Discard only the reviewed version. Any subsequent edit is picked up by refill.
pub(crate) fn review_discard(ctx: &mut Ctx) {
    if let Some(drain) = ctx.editor.quit_drain.as_mut() {
        if let Some((id, version)) = drain.reviewing.take() {
            drain.discarded_versions.insert(id, version);
            if drain.queue.front() == Some(&id) { drain.queue.pop_front(); }
        }
    }
    drive(ctx);
}

/// Save the reviewed buffer, not whichever buffer might have become active meanwhile.
/// Re-display a stale review before accepting a decision about its newer version.
pub(crate) fn review_save(ctx: &mut Ctx) {
    let reviewed = ctx.editor.quit_drain.as_mut().and_then(|d| d.reviewing.take());
    let Some((id, version)) = reviewed else { drive(ctx); return; };
    let idx = ctx.editor.buffers.iter().position(|b| b.id == id && b.document.version == version);
    let Some(idx) = idx else { drive(ctx); return; };
    crate::workspace::switch_to(ctx.editor, idx);
    crate::save::dispatch_save_then(ctx, PostSaveAction::ContinueQuitDrain);
}

/// Abort an attempted quit, including its awaited completion, without cancelling writes.
pub(crate) fn cancel(editor: &mut Editor) {
    editor.quit = false; // also revoke a provisional pre-callback exit
    editor.quit_drain = None;
    editor.quit_drain_advance = false;
    if let Some(fb) = editor.file_browser.as_mut() { fb.quit_save_owner = None; }
    if editor.pending_after_save.as_ref().is_some_and(|p|
        matches!(p.action, PostSaveAction::ContinueQuitDrain)) {
        editor.pending_after_save = None;
    }
}

/// True while a session-wide quit owns decisions or an awaited filename/write.
pub(crate) fn in_progress(editor: &Editor) -> bool {
    editor.quit || editor.quit_drain.is_some()
        || editor.pending_after_save.as_ref().is_some_and(|p| p.action == PostSaveAction::ContinueQuitDrain)
        || editor.pending_save_as == Some(PostSaveAction::ContinueQuitDrain)
}

/// A manual Save As cannot introduce a new destination while quit is in progress.
pub(crate) fn allow_manual_save_as(editor: &mut Editor) -> bool {
    if !in_progress(editor) { return true; }
    editor.set_status_full(crate::status::StatusKind::Warning, "Save As is unavailable while quitting",
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
    false
}

/// Validate the ownership of a picker, including one opened before quitting began.
pub(crate) fn allow_save_as_picker(editor: &mut Editor, owner: Option<BufferId>) -> bool {
    let Some(id) = owner else { return allow_manual_save_as(editor); };
    if editor.pending_save_as == Some(PostSaveAction::ContinueQuitDrain)
        && editor.active().id == id
        && editor.quit_drain.as_ref().is_some_and(|d| d.queue.front() == Some(&id)) {
        return true;
    }
    editor.pending_save_as = None;
    cancel(editor);
    editor.set_status_full(crate::status::StatusKind::Warning, "buffer changed — save and quit cancelled",
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
    false
}

/// Revalidate at the write boundary too: overwrite prompts can outlive the picker.
pub(crate) fn allow_save_as_write(editor: &mut Editor) -> bool {
    if editor.pending_save_as == Some(PostSaveAction::ContinueQuitDrain) {
        let owner = editor.quit_drain.as_ref().and_then(|d| d.queue.front().copied());
        if owner.is_some() { return allow_save_as_picker(editor, owner); }
        editor.pending_save_as = None;
        cancel(editor);
        editor.set_status_full(crate::status::StatusKind::Warning, "Save As request expired — save cancelled",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return false;
    }
    allow_manual_save_as(editor)
}

/// A clean workspace still waits for the metadata merges of outstanding saves.
/// If typing occurs while waiting, ordinary Quit asks for review rather than saving implicitly.
pub(crate) fn wait_for_saves(editor: &mut Editor, now: u64) -> bool {
    if editor.saves_in_flight.is_empty() { return false; }
    let drain = editor.quit_drain.get_or_insert_with(|| QuitDrain::new(Default::default(), QuitMode::ReviewEach));
    drain.waiting_since.get_or_insert(now);
    editor.set_status(crate::status::StatusKind::Info, "Waiting for saves before quitting");
    true
}

/// Mark a save terminal; return whether its failure cancelled a plain-Quit wait so
/// the completion can explain that cancellation alongside the underlying error.
pub(crate) fn save_finished(editor: &mut Editor, request: crate::save::SaveRequest, succeeded: bool) -> bool {
    let tracked = editor.saves_in_flight.remove(&request);
    if tracked && !succeeded && waiting(editor) {
        // An ordinary Quit waiting for background saves must not exit on their error.
        // An explicitly awaited different request remains protected from this failure.
        cancel(editor);
        return true;
    }
    false
}

fn waiting(editor: &Editor) -> bool {
    editor.quit_drain.as_ref().is_some_and(|d| d.queue.is_empty())
        && editor.pending_after_save.is_none() && editor.pending_save_as.is_none()
}

/// Completion of an unowned/background save can unblock a drained quit flow.
pub(crate) fn wake_if_waiting(editor: &mut Editor) {
    if waiting(editor) { editor.quit_drain_advance = true; }
}

/// Final exit barrier, after callbacks that can edit or dispatch more saves. Preserve
/// the drain's discard decisions until this point; a pre-pump quit is only provisional.
pub(crate) fn after_callbacks(ctx: &mut Ctx) -> bool {
    if !ctx.editor.quit { return true; }
    ctx.editor.quit = false;
    ctx.editor.quit_drain.get_or_insert_with(|| QuitDrain::new(Default::default(), QuitMode::ReviewEach));
    drive(ctx);
    if ctx.editor.quit { ctx.editor.quit_drain = None; }
    !ctx.editor.quit
}
