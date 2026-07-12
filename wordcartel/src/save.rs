//! Background save (spec §4.3). The foreground captures an O(1) rope snapshot +
//! version + path and dispatches a JobKind::Save job; the worker materializes
//! the snapshot off the keystroke path and atomically writes it; the merge
//! updates status/saved_version version-awarely.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::commands::CommandResult;
use crate::file::{self, SaveOutcome};
use crate::jobs::{Job, JobKind, JobResult, ResultClass};
use crate::registry::Ctx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileFingerprint {
    pub mtime: Option<SystemTime>,
    pub size: u64,
    /// FNV-style content discriminator: catches same-size/same-mtime edits that
    /// would otherwise produce an identical (mtime, size) fingerprint on
    /// filesystems with coarse mtime granularity (ext3, FAT, HFS+, etc.).
    pub hash: u64,
}

/// Content-hash fingerprint of `path` for external-modification detection (BUG-2),
/// capping the content read at `MAX_OPEN_BYTES`. Returns `None` only when `path` is
/// missing/unstattable; a present file always yields `Some` (over-cap → mtime+size
/// with a sentinel hash — never `None`, so `stored_fp` can't silently disable the
/// conflict check). `mtime`/`size` come from `metadata`, `hash` from a separate
/// bounded read (no single-syscall guarantee across the three fields).
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    fingerprint_with_limit(path, crate::limits::MAX_OPEN_BYTES)
}

/// Content-hash fingerprint, capping the content read at `limit`. `meta` failure
/// (missing/unreadable) → `None`; but a present-but-over-cap file yields a
/// metadata-only fingerprint (real mtime+size, sentinel hash 0) rather than `None`,
/// so `stored_fp` never becomes `None` and the external-mod check is not silently
/// defeated (`None == None`) for files grown beyond the cap.
fn fingerprint_with_limit(path: &Path, limit: u64) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let hash = match crate::file::bounded_read_opt(path, limit) {
        Some(bytes) => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hasher::write(&mut h, &bytes);
            std::hash::Hasher::finish(&h)
        }
        None => 0, // over-cap (or transient read failure): fall back to mtime+size only
    };
    Some(FileFingerprint { mtime: meta.modified().ok(), size: meta.len(), hash })
}

/// Whether a save job writes the document's own path (Normal) or re-keys the
/// buffer onto a new `target` on success (SaveAs). `Copy` → free to `matches!`
/// repeatedly inside the moved merge closure (Codex).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode { Normal, SaveAs }

/// Internal: dispatch the save job (no external-mod check). `do_save` delegates
/// here with `SaveMode::Normal`; Save-As enters with `SaveMode::SaveAs` and the
/// re-key `target`. Called by `dispatch_save`/`overwrite_save` (Normal) and
/// `perform_save_as` (SaveAs).
pub(crate) fn do_save_to(ctx: &mut Ctx, target: std::path::PathBuf, mode: SaveMode) {
    // §3.9: status BEFORE dispatch. O(1) snapshot; version captured now.
    ctx.editor.status = "Saving\u{2026}".to_string();
    let snap = ctx.editor.active().document.buffer.snapshot(); // O(1) ropey clone
    let v = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;
    let prior_key = ctx.editor.active().document.path.clone(); // for SaveAs swap re-key
    let write_path = target.clone();

    ctx.executor.dispatch(Job {
        buffer_id,
        class: ResultClass::Durability,
        version: v,
        kind: JobKind::Save,
        run: Box::new(move || {
            // Worker: materialize the snapshot off the keystroke path, then write.
            let content = snap.to_string();
            let outcome = file::save_atomic(&write_path, &content);
            let new_fp = fingerprint(&write_path);
            JobResult {
                buffer_id,
                class: ResultClass::Durability,
                version: v,
                kind: JobKind::Save,
                merge: Box::new(move |editor| {
                    // INVARIANT: route via by_id_mut(buffer_id) — NEVER active(); the merge must
                    // target the originating buffer even after a buffer switch (multi-buffer, Effort 6).
                    // Assemble the (global) status in a local so the `b` mutable borrow ends
                    // before we touch editor.status.
                    // P2 on_save fire site: computed from the closure's OWNED `target` (the
                    // written path), NOT from `b` — a closed buffer must still fire (the write
                    // DID succeed). Fires on Saved AND Unchanged (both are the user-visible
                    // "a save completed" outcome); Err fires nothing.
                    let fire_save: Option<PathBuf> =
                        matches!(outcome, Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged))
                            .then(|| target.clone());
                    let mut status = String::new();
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        match outcome {
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                if matches!(mode, SaveMode::SaveAs) { b.document.path = Some(target.clone()); }
                                b.document.saved_version = Some(v);
                                b.document.stored_fp = new_fp;
                                // The swap latch (`swapped_version`) asserts "this version's content
                                // is in the swap file". A successful save deletes/rekeys that swap
                                // (below), so clear the latch — otherwise a later same-version dirty
                                // state would read as "already swapped" and skip writing a fresh swap
                                // (Codex pre-merge). Clearing errs toward writing a swap (durability-safe)
                                // and re-arms the expedited SaveAs-still-editing checkpoint.
                                b.swapped_version = None;
                                if b.document.version == v {
                                    status = "Saved".to_string();
                                    crate::swap::delete(b.document.path.as_deref());
                                    if matches!(mode, SaveMode::SaveAs) { crate::swap::delete(prior_key.as_deref()); }
                                } else {
                                    status = format!("Saved v{v} (still editing)");
                                    // Staged re-key (Codex): the buffer was edited during the write
                                    // (now v+1). Delete the prior/scratch swap (its v content is now
                                    // ON DISK at `target`, and leaving a scratch swap would trigger a
                                    // spurious recovery next launch) and EXPEDITE a swap under the new
                                    // path: `last_swap_at = None` makes the next `due()` fire promptly,
                                    // writing a swap for the v+1 body under `target`. Exposure for the
                                    // v→v+1 keystrokes is bounded by the normal swap cadence (the same
                                    // window normal editing has between periodic swap writes).
                                    if matches!(mode, SaveMode::SaveAs) {
                                        crate::swap::delete(prior_key.as_deref());
                                        b.last_swap_at = None;
                                    }
                                }
                            }
                            Err(e) => {
                                // Failure: leave saved_version/stored_fp/path untouched
                                // (buffer stays dirty; SaveAs path stays None/old); surface the error.
                                // Do NOT re-introduce SaveError::Symlink match — e.to_string()
                                // for Symlink still contains "symlink", satisfying the test.
                                status = e.to_string();
                            }
                        }
                    }
                    editor.status = status;
                    // Fire AFTER the by_id_mut block closes (never inside it — a live `b: &mut
                    // Buffer` borrow would conflict with fire_event's `&mut Editor`) and after
                    // editor.status is set — mirrors the local-then-assign shape above.
                    if let Some(p) = fire_save {
                        crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::Save, Some(&p));
                    }
                }),
            }
        }),
    });
}

/// Internal: dispatch a Normal save of the document's own path (no external-mod
/// check). Called by `dispatch_save` (after the check) and `overwrite_save`.
fn do_save(ctx: &mut Ctx) {
    let path = ctx.editor.active().document.path.clone().expect("do_save called without a path");
    do_save_to(ctx, path, SaveMode::Normal);
}

/// Registry `"save"` handler: external-mod check then dispatch a background save.
pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult {
    let path = match &ctx.editor.active().document.path {
        None => { crate::prompts::open_save_as(ctx.editor); return CommandResult::Handled; }
        Some(p) => p.clone(),
    };

    // External-mod check (§4.3 step 2): cheap stat; if the on-disk fingerprint
    // diverged from what we last wrote, refuse and raise the external-mod modal.
    let current_fp = fingerprint(&path);
    if current_fp != ctx.editor.active().document.stored_fp {
        ctx.editor.open_prompt(crate::prompt::Prompt::external_mod());
        ctx.editor.status =
            "File changed on disk \u{2014} choose [R]eload or [O]verwrite".to_string();
        return CommandResult::Handled;
    }

    do_save(ctx);
    CommandResult::Handled
}

/// The unified "save, then do `action`" entry. Goes through `dispatch_save`
/// (external-mod-checked). Handles all three buffer states:
/// - NAMED, no conflict → a save job is dispatched → arm `pending_after_save{action}`.
/// - NAMED, external-mod conflict → `dispatch_save` raised the modal → do NOT arm
///   (the user resolves the modal and re-issues).
/// - UNNAMED → `dispatch_save` opened the Save-As minibuffer → carry the action in
///   `pending_save_as` so it fires after the Save-As write completes.
pub(crate) fn dispatch_save_then(ctx: &mut crate::registry::Ctx, action: crate::editor::PostSaveAction) {
    let was_unnamed = ctx.editor.active().document.path.is_none();
    let buffer_id = ctx.editor.active().id;
    let v = ctx.editor.active().document.version;
    dispatch_save(ctx);
    if was_unnamed {
        // dispatch_save opened Save-As (MinibufferKind::SaveAs) for the no-path buffer.
        if ctx.editor.minibuffer.as_ref().map(|m| m.kind) == Some(crate::minibuffer::MinibufferKind::SaveAs) {
            ctx.editor.pending_save_as = Some(action);
        }
    } else if ctx.editor.active().document.path.is_some() && ctx.editor.prompt.is_none() {
        ctx.editor.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id, version: v, action, at_ms: ctx.clock.now_ms(),
        });
    }
}

/// Save, then quit once the save completes. Delegates to `dispatch_save_then`.
pub(crate) fn dispatch_save_and_quit(ctx: &mut crate::registry::Ctx) {
    dispatch_save_then(ctx, crate::editor::PostSaveAction::Quit);
}

/// Save bypassing the fingerprint conflict (the [O]verwrite modal action).
pub fn overwrite_save(ctx: &mut Ctx) {
    if ctx.editor.active().document.path.is_none() {
        ctx.editor.status = "No file name — use Save As".to_string();
        return;
    }
    do_save(ctx); // no stat check
}

/// [R]eload: discard in-memory edits, reload F from disk. Destructive — only
/// reachable via the external-mod modal. Sanctioned whole-document replacement
/// (fresh Document, not `apply`): there is no incremental delta and history is reset.
pub fn reload_from_disk(editor: &mut crate::editor::Editor) {
    let Some(path) = editor.active().document.path.clone() else { return };
    let text = match crate::file::open(&path) {
        Ok(t) => t,
        Err(e) => { editor.status = e.to_string(); return; }
    };
    // Fix A1: capture the previous version BEFORE replacing the buffer so we
    // can carry it forward.  A diagnostics check in flight before the reload
    // was stamped with `previous_version` (or earlier); the new buffer must
    // start at `previous_version + 1` so any late pre-reload result can never
    // match the version gate in `apply_diagnostics_done`.
    let previous_version = editor.active().document.version;
    let area = editor.active().view.area;
    let fresh = crate::editor::Editor::new_from_text(&text, Some(path.clone()), area); // saved_version=Some(0) → clean
    let mut new_buf = fresh.buffers.into_iter().next().expect("new_from_text yields one buffer");
    // Bump version past the pre-reload value so stale diagnostics results can't match.
    new_buf.document.version = previous_version + 1;
    // Preserve saved_version relative to the new version: the file we just
    // loaded IS the on-disk content, so mark it as saved at the new version.
    new_buf.document.saved_version = Some(previous_version + 1);
    // Reset the DiagStore so no stale underlines from the old content persist.
    new_buf.diagnostics = crate::diagnostics_run::DiagStore::new();
    // Fresh buffer is full-parsed for version 0; the version bump above skips
    // that origin — sync blocks_version so rebuild skips the redundant reparse.
    new_buf.reconcile.blocks_version = new_buf.document.version;
    let id = editor.active().id;                 // preserve THIS buffer's id
    // Effort A: abandon the pre-reload generation before the wholesale replace so a still-in-transit
    // publish for the old content is dropped (its uri leaves uri_owner) and the next Review dispatch
    // reopens fresh. The version bump above is the second, independent guard axis (spec §5 item 4).
    editor.diag_providers.notify_close_all(id);
    // 5g: capture folds before replacement so we can carry them forward.
    let prev_folded = editor.active().folds.folded().clone();
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    // 5g: carry folds across the reload and reconcile against the new tree.
    editor.active_mut().folds.replace_folded(prev_folded);
    // Clear any stale search/diag overlay — the buffer content has changed wholesale.
    editor.search = None;
    editor.diag = None;
    // then the existing follow-ups, now on the active buffer:
    editor.active_mut().invalidate_layout();
    crate::derive::rebuild(editor); // reconciles folds + normalizes scroll
    // normalize the caret out of any fold the new content created/changed.
    let head = editor.active().document.selection.primary().head;
    let nc = {
        let b = editor.active();
        crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, head)
    };
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
    crate::nav::ensure_visible(editor);
    editor.active_mut().document.stored_fp = fingerprint(&path);
    editor.status = "Reloaded".into();
    crate::swap::delete(editor.active().document.path.as_deref());
}

/// Load recovered swap content into the buffer; keep the path; mark dirty.
/// Sanctioned whole-document replacement (fresh Document, history reset).
pub fn load_recovered(editor: &mut crate::editor::Editor, body: &str) {
    let path = editor.active().document.path.clone();
    // Fix A1: capture the previous version BEFORE replacing the buffer so we
    // can carry it forward, preventing late pre-reload diagnostics results
    // from matching the version gate in `apply_diagnostics_done`.
    let previous_version = editor.active().document.version;
    let area = editor.active().view.area;
    let fresh = crate::editor::Editor::new_from_text(body, path.clone(), area);
    let mut new_buf = fresh.buffers.into_iter().next().expect("new_from_text yields one buffer");
    // Bump version past the pre-reload value; recovered content is unsaved.
    new_buf.document.version = previous_version + 1;
    new_buf.document.saved_version = None; // recovered work is unsaved
    // Reset the DiagStore so no stale underlines from the old content persist.
    new_buf.diagnostics = crate::diagnostics_run::DiagStore::new();
    // Fresh buffer is full-parsed for version 0; the version bump above skips
    // that origin — sync blocks_version so rebuild skips the redundant reparse.
    new_buf.reconcile.blocks_version = new_buf.document.version;
    let id = editor.active().id;                 // preserve THIS buffer's id
    // Effort A: abandon the pre-recovery generation before the wholesale replace (same guard as
    // reload_from_disk) — the in-transit old-content publish is dropped and the buffer reopens fresh.
    editor.diag_providers.notify_close_all(id);
    // 5g: capture folds before replacement so we can carry them forward.
    let prev_folded = editor.active().folds.folded().clone();
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    // 5g: carry folds across the recovery and reconcile against the new tree.
    editor.active_mut().folds.replace_folded(prev_folded);
    // Clear any stale search/diag overlay — the buffer content has changed wholesale.
    editor.search = None;
    editor.diag = None;
    editor.active_mut().invalidate_layout();
    crate::derive::rebuild(editor); // reconciles folds + normalizes scroll
    // normalize the caret out of any fold the new content created/changed.
    let head = editor.active().document.selection.primary().head;
    let nc = {
        let b = editor.active();
        crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, head)
    };
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
    crate::nav::ensure_visible(editor);
    editor.active_mut().document.stored_fp = path.as_deref().and_then(fingerprint);
    editor.status = "Recovered unsaved changes".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::registry::Ctx;
    use wordcartel_core::history::Clock;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }
    static SEQ: AtomicU32 = AtomicU32::new(0);
    fn tx() -> std::sync::mpsc::Sender<crate::app::Msg> {
        std::sync::mpsc::channel().0
    }
    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wcartel-bgsave-{}-{}.md",
            std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
    }

    #[test]
    fn background_save_clears_dirty_at_saved_version() {
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; // simulate an unsaved edit
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() };
            dispatch_save(&mut ctx);
        }
        assert_eq!(e.status, "Saving\u{2026}", "status set before dispatch (§3.9)");
        // InlineExecutor already ran the job; apply the buffered merge.
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty(), "version==saved_version after save → clean");
        assert_eq!(e.status, "Saved");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn background_save_result_for_old_version_does_not_mark_clean() {
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("v1\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        // User edits on to version 2 BEFORE the merge applies.
        e.active_mut().document.version = 2;
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        // saved_version recorded v1, but the buffer is at v2 → still dirty.
        assert_eq!(e.active().document.saved_version, Some(1));
        assert!(e.active().document.dirty(), "edited-on buffer stays dirty after a stale-version save");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn background_save_failure_keeps_dirty_and_status() {
        // Save through a symlink is refused by save_atomic → merge must keep dirty.
        let real = scratch();
        let link = scratch();
        std::fs::write(&real, "real\n").unwrap();
        #[cfg(unix)] std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))] { let _ = &link; return; }
        let mut e = Editor::new_from_text("x\n", Some(link.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.active().document.dirty(), "failed save must leave the buffer dirty");
        assert!(e.active().document.saved_version.is_none());
        assert!(e.status.to_lowercase().contains("symlink"));
        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
    }

    /// A real (non-symlink) save failure also keeps the buffer dirty: the target's
    /// parent is a regular FILE, so temp creation fails immediately (parent not a
    /// directory). Drives the do_save_to merge's Err arm without any test seam.
    #[cfg(unix)]
    #[test]
    fn background_save_failure_parent_is_file_keeps_dirty() {
        let parent = scratch();
        std::fs::write(&parent, "i am a file, not a dir\n").unwrap();
        // target sits "inside" a regular file → ENOTDIR on temp create.
        let target = parent.join("doc.md");

        let mut e = Editor::new_from_text("hello\n", Some(target.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert!(e.active().document.dirty(), "failed save must leave the buffer dirty");
        assert!(e.active().document.saved_version.is_none());
        // Assert the ENOTDIR error itself surfaced (not merely any non-empty status) — this
        // proves the save was attempted and failed, NOT that the external-mod modal opened.
        assert!(
            e.status.to_lowercase().contains("not a directory") || e.status.contains("ENOTDIR"),
            "OS ENOTDIR must surface as status; got: {:?}", e.status
        );
        let _ = std::fs::remove_file(&parent);
    }

    #[test]
    fn save_clean_deletes_swap_but_stale_save_keeps_it() {
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();

        // Pre-create a swap for this doc.
        let sp = crate::swap::swap_path(Some(&p)).unwrap();
        crate::swap::write_atomic(&sp, "stub").unwrap();
        assert!(sp.exists());

        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty());
        assert!(!sp.exists(), "a save that leaves the buffer clean deletes the swap");

        // Now: dispatch a save at v2, but edit on to v3 before the merge → keep swap.
        crate::swap::write_atomic(&sp, "stub2").unwrap();
        e.active_mut().document.version = 2;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        e.active_mut().document.version = 3; // edited on
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.active().document.dirty());
        assert!(sp.exists(), "a stale-version save must NOT delete the swap");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dispatch_save_raises_modal_on_external_change() {
        use crate::jobs::InlineExecutor;
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "v0\n").unwrap();
        let mut e = Editor::new_from_text("mine\n", Some(p.clone()), (80, 24));
        // stored_fp captured at load == v0's fp. Now an external process rewrites F.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&p, "external change\n").unwrap();
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "external change must raise the modal, not clobber");
        assert!(ex.drain().is_empty(), "no save job dispatched on conflict");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fingerprint_matrix_new_and_deleted_are_conflicts() {
        // New named buffer (stored_fp = None) but a file now exists → conflict.
        let p = scratch();
        std::fs::write(&p, "created externally\n").unwrap();
        let mut e = Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
        e.active_mut().document.stored_fp = None;        // "did not exist at load"
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = crate::jobs::InlineExecutor::default();
        let clk = Z;
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() };
          dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "a file appearing where there was none is a conflict");
        assert!(ex.drain().is_empty(), "no save job dispatched on new-file conflict");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn overwrite_save_bypasses_the_stat_check() {
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "v0\n").unwrap();
        let mut e = Editor::new_from_text("mine\n", Some(p.clone()), (80, 24));
        std::fs::write(&p, "external\n").unwrap(); // diverged
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; overwrite_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine\n", "overwrite wins");
        assert!(!e.active().document.dirty());
        assert_eq!(e.active().document.stored_fp, crate::save::fingerprint(&p), "overwrite refreshes stored_fp");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn reload_from_disk_resets_to_file_and_marks_clean() {
        let p = scratch();
        std::fs::write(&p, "on disk\n").unwrap();
        let mut e = Editor::new_from_text("in memory edits\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 4; e.active_mut().document.saved_version = None;
        reload_from_disk(&mut e);
        assert_eq!(e.active().document.buffer.to_string(), "on disk\n");
        assert!(!e.active().document.dirty(), "reloaded buffer is clean");
        assert_eq!(e.active().document.stored_fp, crate::save::fingerprint(&p), "reload refreshes stored_fp");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn reload_from_disk_clears_open_search_overlay() {
        let p = scratch();
        std::fs::write(&p, "fresh content\n").unwrap();
        let mut e = Editor::new_from_text("in memory edits\n", Some(p.clone()), (80, 24));
        // Open a search overlay so we can assert it gets cleared.
        e.open_search(crate::search_overlay::Phase::Find, 0);
        assert!(e.search.is_some(), "search overlay open before reload");
        reload_from_disk(&mut e);
        assert!(e.search.is_none(), "reload_from_disk must clear any open search overlay");
        assert_eq!(e.active().document.buffer.to_string(), "fresh content\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dispatch_save_refuses_when_file_changed_on_disk() {
        let p = scratch();
        std::fs::write(&p, "original\n").unwrap();
        // Editor loads the file → stored_fp captured at load.
        let mut e = Editor::new_from_text("my edits\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        // External process rewrites the file after load (different size → fingerprint differs).
        std::fs::write(&p, "changed externally, much longer line\n").unwrap();
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() };
            dispatch_save(&mut ctx);
        }
        assert!(ex.drain().is_empty(), "no save job dispatched on external-mod conflict");
        assert!(e.status.to_lowercase().contains("changed on disk"), "status surfaces the refusal");
        assert!(e.active().document.dirty(), "buffer stays dirty when a save is refused");
        let _ = std::fs::remove_file(&p);
    }

    // -----------------------------------------------------------------------
    // Fix A1: reload version-bump + DiagStore reset
    // -----------------------------------------------------------------------

    /// A late DiagnosticsDone result for the pre-reload version must be DISCARDED
    /// after reload_from_disk, because the reload bumps the version past V.
    #[test]
    fn reload_discards_pre_reload_diagnostics_done() {
        let p = scratch();
        std::fs::write(&p, "new content\n").unwrap();
        let mut e = Editor::new_from_text("old content\n", Some(p.clone()), (80, 24));
        // Version V; arm a fake in-flight diagnostics check.
        let pre_reload_version = e.active().document.version; // 0
        let harper = wordcartel_core::diagnostics::DiagSource::Harper;
        e.active_mut().diagnostics.slot_mut(harper).in_flight_version = Some(pre_reload_version);
        e.active_mut().diagnostics.slot_mut(harper).diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "fake".into(),
                suggestions: vec![],
            }
        ];
        e.active_mut().diagnostics.slot_mut(harper).computed_version = pre_reload_version;
        reload_from_disk(&mut e);
        // Post-reload: version must be strictly greater than pre_reload_version.
        assert!(e.active().document.version > pre_reload_version,
            "reload must bump version past the pre-reload value");
        // DiagStore must be cleared of any stale underlines.
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.diagnostics.is_empty()),
            "reload must reset DiagStore (no stale underlines)");
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.in_flight_version.is_none()),
            "reload must clear in_flight_version");
        // Now deliver the late DiagnosticsDone for the pre-reload version V.
        let new_version = e.active().document.version;
        let buffer_id = e.active().id;
        crate::diagnostics_run::apply_diagnostics_done(
            &mut e,
            buffer_id,
            pre_reload_version, // stale version
            wordcartel_core::diagnostics::DiagSource::Harper,
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "stale".into(),
                suggestions: vec![],
            }],
        );
        // The stale result must NOT be stored.
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.diagnostics.is_empty()),
            "late pre-reload DiagnosticsDone must be discarded (version gate)");
        // computed_version must not have been set to the new buffer's version by the stale result
        // (i.e., the new buffer's version != pre_reload_version, and diagnostics are still empty).
        assert_ne!(e.active().document.version, pre_reload_version,
            "new buffer must have a different version than the pre-reload snapshot");
        // Sanity: a fresh result for the new version IS accepted.
        crate::diagnostics_run::apply_diagnostics_done(
            &mut e,
            buffer_id,
            new_version,
            wordcartel_core::diagnostics::DiagSource::Harper,
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "fresh".into(),
                suggestions: vec![],
            }],
        );
        assert_eq!(e.active().diagnostics.slot(harper).unwrap().diagnostics.len(), 1,
            "fresh result for the new version must be stored");
        let _ = std::fs::remove_file(&p);
    }

    /// Same invariant for load_recovered: a late DiagnosticsDone for the
    /// pre-recovery version is discarded after the version bump.
    #[test]
    fn load_recovered_discards_pre_recovery_diagnostics_done() {
        let mut e = Editor::new_from_text("old content\n", None, (80, 24));
        let pre_recovery_version = e.active().document.version; // 0
        let harper = wordcartel_core::diagnostics::DiagSource::Harper;
        e.active_mut().diagnostics.slot_mut(harper).in_flight_version = Some(pre_recovery_version);
        // Simulate stale underlines that must be wiped by recovery.
        e.active_mut().diagnostics.slot_mut(harper).diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "old".into(),
                suggestions: vec![],
            }
        ];
        e.active_mut().diagnostics.slot_mut(harper).computed_version = pre_recovery_version;
        load_recovered(&mut e, "recovered content\n");
        assert!(e.active().document.version > pre_recovery_version,
            "load_recovered must bump version past the pre-recovery value");
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.diagnostics.is_empty()),
            "load_recovered must reset DiagStore");
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.in_flight_version.is_none()),
            "load_recovered must clear in_flight_version");
        let buffer_id = e.active().id;
        // Deliver a late result for the pre-recovery version — must be discarded.
        crate::diagnostics_run::apply_diagnostics_done(
            &mut e,
            buffer_id,
            pre_recovery_version,
            wordcartel_core::diagnostics::DiagSource::Harper,
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "stale".into(),
                suggestions: vec![],
            }],
        );
        assert!(e.active().diagnostics.slot(harper).is_none_or(|s| s.diagnostics.is_empty()),
            "late pre-recovery DiagnosticsDone must be discarded");
    }

    /// Effort A: reload/recover close the pre-replacement generation on the provider (the
    /// generation-axis half of the double staleness guard, spec §5 item 4) so an in-transit
    /// old-content publish is dropped and the buffer reopens fresh.
    #[test]
    fn reload_and_recover_notify_provider_close() {
        use crate::diag_provider::{RecordingProvider, ProviderCall};
        // reload_from_disk
        let p = scratch();
        std::fs::write(&p, "on disk\n").unwrap();
        let mut e = Editor::new_from_text("edits\n", Some(p.clone()), (80, 24));
        let id = e.active().id;
        let rec = RecordingProvider::new().with_source(wordcartel_core::diagnostics::DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        reload_from_disk(&mut e);
        assert!(calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::NotifyClose(x) if *x == id)),
            "reload_from_disk notifies close for the pre-reload generation");
        let _ = std::fs::remove_file(&p);

        // load_recovered
        let mut e = Editor::new_from_text("old\n", None, (80, 24));
        let id = e.active().id;
        let rec = RecordingProvider::new().with_source(wordcartel_core::diagnostics::DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        load_recovered(&mut e, "recovered\n");
        assert!(calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::NotifyClose(x) if *x == id)),
            "load_recovered notifies close for the pre-recovery generation");
    }

    #[test]
    fn reload_reconciles_folds_against_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.md");
        std::fs::write(&path, "## A\nbody\n## B\nx\n").unwrap();
        let mut ed = crate::editor::Editor::new_from_text("## A\nbody\n## B\nx\n", Some(path.clone()), (80, 24));
        crate::derive::rebuild(&mut ed);
        ed.active_mut().folds.toggle(0); // fold ## A (byte 0, survives rewrite)
        ed.active_mut().folds.toggle("## A\nbody\n".len()); // fold ## B
        // rewrite the file so ## B is gone
        std::fs::write(&path, "## A\nbody only\n").unwrap();
        let b_anchor = "## A\nbody\n".len(); // the ## B offset we folded
        crate::save::reload_from_disk(&mut ed);
        // STRONG assertion: ## A is still a heading at byte 0 — its fold must be preserved
        assert!(ed.active().folds.folded().contains(&0), "## A still exists after reload — its fold must be preserved");
        // STRONG assertion: the exact stale ## B anchor is gone, and the surviving
        // fold set equals exactly the post-reconcile heading-start set it should be.
        assert!(!ed.active().folds.folded().contains(&b_anchor), "stale ## B fold must be dropped");
        let starts: std::collections::BTreeSet<usize> = {
            let b = ed.active();
            wordcartel_core::outline::heading_starts(b.document.blocks(), &b.document.buffer.snapshot())
        };
        assert!(ed.active().folds.folded().iter().all(|b| starts.contains(b)),
            "every surviving fold must be a real heading start in the new content");
        // caret is visible (normalize is a no-op because it's already out of folds)
        let head = ed.active().document.selection.primary().head;
        let b = ed.active();
        assert_eq!(crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, head), head);
    }

    // -----------------------------------------------------------------------
    // BUG-2 regression: content discriminator (hash) catches same-size/same-mtime edits
    // -----------------------------------------------------------------------

    #[test]
    fn fingerprint_over_cap_falls_back_to_metadata_not_none() {
        let p = scratch();
        std::fs::write(&p, b"0123456789").unwrap(); // 10 bytes
        let fp = fingerprint_with_limit(&p, 4).expect("over-cap present file still yields a fingerprint");
        assert_eq!(fp.size, 10, "size from metadata");
        assert_eq!(fp.hash, 0, "over-cap → sentinel hash, NOT None (closes None==None)");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fingerprint_within_cap_hashes_content_unchanged() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").unwrap();
        let within = fingerprint_with_limit(&p, 1_000_000).expect("fp");
        assert_ne!(within.hash, 0, "≤cap → real content hash");
        assert_eq!(within.hash, fingerprint(&p).unwrap().hash, "≤cap path identical to public fingerprint (no churn)");
        let _ = std::fs::remove_file(&p);
    }

    /// Regression for BUG-2: a same-byte-count external edit that lands within the
    /// same mtime tick must still be detected as a change.  The hash field in
    /// `FileFingerprint` provides the content discriminator that (mtime, size) alone
    /// cannot.
    #[test]
    fn fingerprint_detects_same_size_different_content() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").unwrap();
        let fp_a = fingerprint(&p).expect("fingerprint of 'aaaa' file");

        // Overwrite with same-length content — no sleep, so mtime may be identical.
        std::fs::write(&p, b"bbbb").unwrap();
        let fp_b = fingerprint(&p).expect("fingerprint of 'bbbb' file");

        // Size is the same (4 bytes); the hash must differ so the fingerprints are !=.
        assert_eq!(fp_a.size, fp_b.size, "sizes must match for this to be a meaningful test");
        assert_ne!(fp_a.hash, fp_b.hash, "hash must differ for different content");
        assert_ne!(fp_a, fp_b,
            "FileFingerprint must detect same-size/same-mtime external content change (BUG-2)");

        let _ = std::fs::remove_file(&p);
    }

    /// BUG-2 integration: dispatch_save must raise the external-mod modal when an
    /// external process rewrites the file with same-length content (no mtime change
    /// guaranteed).  Without the hash field this would silently overwrite the edit.
    #[test]
    fn dispatch_save_raises_modal_on_same_size_external_change() {
        let p = scratch();
        // Write exactly 4-byte initial content.
        std::fs::write(&p, b"aaaa").unwrap();
        let mut e = Editor::new_from_text("mine\n", Some(p.clone()), (80, 24));
        // stored_fp is now fingerprint of "aaaa".  Simulate a same-size external edit
        // (no sleep — we deliberately do NOT wait for a new mtime tick).
        std::fs::write(&p, b"bbbb").unwrap();
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() };
            dispatch_save(&mut ctx);
        }
        assert!(e.prompt.is_some(),
            "same-size external content change must raise the external-mod modal (BUG-2)");
        assert!(ex.drain().is_empty(),
            "no save job must be dispatched when same-size external change is detected");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn panicked_save_keeps_dirty_and_aborts_quit() {
        let p = scratch(); std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("v1\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let id = e.active().id;
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: id, version: 1, action: crate::editor::PostSaveAction::Quit, at_ms: 0 });
        // An active quit drain must be ABORTED (not stranded) when the awaited save panics.
        e.quit_drain = Some(crate::editor::QuitDrain {
            queue: std::collections::VecDeque::new(), mode: crate::editor::QuitMode::SaveAll });
        e.quit_drain_advance = true;
        crate::jobs_apply::apply_outcome(
            crate::jobs::JobOutcome::Panicked {
                buffer_id: id, version: 1, kind: crate::jobs::JobKind::Save, msg: "boom".into() },
            &mut e);
        assert!(e.active().document.dirty(), "panicked save keeps the buffer dirty");
        assert!(e.pending_after_save.is_none(), "awaited quit must be cleared");
        assert!(e.quit_drain.is_none(), "the quit drain must be aborted, not stranded");
        assert!(!e.quit_drain_advance, "quit_drain_advance must be reset");
        assert!(!e.quit, "must NOT quit on a panicked save");
        assert!(e.status.to_lowercase().contains("save"));
        let _ = std::fs::remove_file(&p);
    }
}
