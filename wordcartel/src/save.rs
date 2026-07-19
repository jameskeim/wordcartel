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
    fingerprint_with_fs(&crate::fsx::RealFs, path)
}

/// Seam-taking core of [`fingerprint`]. Kept `pub(crate)` so tests can inject a `FaultFs`.
pub(crate) fn fingerprint_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<FileFingerprint> {
    fingerprint_with_limit(fs, path, crate::limits::MAX_OPEN_BYTES)
}

/// Content-hash fingerprint, capping the content read at `limit`.
///
/// Returns `None` when the path is missing/unstattable — AND when it is a BROKEN symlink,
/// because today's `std::fs::metadata(path).ok()?` fails for a dangling link and the seam's
/// `stat` succeeds for one. Without the explicit `broken` guard this would return `Some`
/// with zeroed fields and silently defeat the external-mod check.
///
/// A present, resolvable but over-cap file still yields a metadata-only fingerprint (real
/// mtime+size, sentinel hash 0) rather than `None`, so `stored_fp` never becomes `None`
/// and `None == None` cannot disable the conflict check.
fn fingerprint_with_limit(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<FileFingerprint>
{
    let st = fs.stat(path).ok()?;
    if st.broken { return None; }
    let hash = match crate::file::bounded_read_opt_with_fs(fs, path, limit) {
        Some(bytes) => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hasher::write(&mut h, &bytes);
            std::hash::Hasher::finish(&h)
        }
        None => 0, // over-cap (or transient read failure): fall back to mtime+size only
    };
    Some(FileFingerprint { mtime: st.mtime, size: st.len, hash })
}

/// Whether a save job writes the document's own path (Normal) or re-keys the
/// buffer onto a new `target` on success (SaveAs). `Copy` → free to `matches!`
/// repeatedly inside the moved merge closure (Codex).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode { Normal, SaveAs }

/// The two paths a save needs, kept apart because a symlinked destination makes them differ.
///
/// A struct rather than two positional `PathBuf`s ON PURPOSE: same-typed positional
/// parameters are silently swappable, and getting this wrong reintroduces either the
/// unsaveable-symlink defect or the canonical-`Document.path` regressions. For an ordinary
/// destination the two fields are equal, which is the common case and costs nothing.
#[derive(Clone, Debug)]
pub(crate) struct SaveTarget {
    /// What the writer selected — logical, possibly a symlink.
    pub chosen: std::path::PathBuf,
    /// Where bytes actually go — resolution applied. Never a symlink.
    pub resolved: std::path::PathBuf,
}

impl SaveTarget {
    /// For a destination that needed no resolution (the common case: the two are equal).
    #[allow(dead_code)] // C5 Task 21 wires Write-Block through this same-target path; forward reference
    pub(crate) fn same(p: std::path::PathBuf) -> Self {
        SaveTarget { chosen: p.clone(), resolved: p }
    }
}

/// Internal: dispatch the save job (no external-mod check). `do_save` delegates
/// here with `SaveMode::Normal`; Save-As enters with `SaveMode::SaveAs` and the
/// re-key `target`. Called by `dispatch_save`/`overwrite_save` (Normal) and
/// `perform_save_as` (SaveAs).
pub(crate) fn do_save_to(ctx: &mut Ctx, target: SaveTarget, mode: SaveMode) {
    // §3.9: status BEFORE dispatch. O(1) snapshot; version captured now.
    let snap = ctx.editor.active().document.buffer.snapshot(); // O(1) ropey clone
    let v = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;
    let prior_key = ctx.editor.active().document.path.clone(); // for SaveAs swap re-key
    let write_path = target.resolved.clone();   // bytes go HERE
    let chosen_path = target.chosen.clone();    // the buffer is rekeyed to THIS
    // OWNED handle cloned into the job closure. `jobs::Job::run` is
    // `Box<dyn FnOnce() -> JobResult + Send>`, so a borrowed `&dyn Fs` cannot cross —
    // which is exactly why Task 5 put an `Arc` on `Ctx`.
    let fs = std::sync::Arc::clone(&ctx.fs);
    // Self-replacing Progress keyed on THIS (buffer, version): the completion below reconstructs the
    // identical key from the captured `buffer_id`/`v` and collapses exactly this start (§4.2). A
    // concurrent same-buffer save at a different version keeps its own lineage; a Filter/Transform
    // finish can never collapse it (exact-match topic).
    ctx.editor.set_progress(crate::status::StatusTopic::Save(buffer_id, v), "Saving\u{2026}");

    ctx.executor.dispatch(Job {
        buffer_id,
        class: ResultClass::Durability,
        version: v,
        kind: JobKind::Save,
        run: Box::new(move || {
            // Worker: materialize the snapshot off the keystroke path, then write.
            let content = snap.to_string();
            // Both the write and the fingerprint use the RESOLVED path: the fingerprint must
            // describe the file actually written. (`fingerprint` follows symlinks, so this
            // agrees with `dispatch_save`'s check on Document.path whenever the link resolves.)
            //
            // BOTH go through the SEAM (`*_with_fs`), not the `RealFs` wrappers. This is the
            // single most valuable thing the seam extension buys: the worker-side save path
            // becomes fault-testable for the FIRST time. Calling `file::save_atomic` here —
            // which hardcodes `RealFs` internally — would silently discard that, and an
            // `Arc<FaultFs>` injected at `Ctx` would have no effect.
            let outcome = file::save_atomic_with_fs(&*fs, &write_path, &content);
            let new_fp = fingerprint_with_fs(&*fs, &write_path);
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
                    // P2 on_save fire site: computed from the closure's OWNED `chosen_path`,
                    // NOT from `b` — a closed buffer must still fire (the write DID succeed).
                    // Fires on Saved AND Unchanged (both are the user-visible "a save completed"
                    // outcome); Err fires nothing.
                    //
                    // CHOSEN, not resolved: consistency with `plugin::api`'s `wc.path()`, which
                    // returns `Document.path`. A Save event reporting a path `wc.path()` never
                    // returns would make the two disagree.
                    let fire_save: Option<PathBuf> =
                        matches!(outcome, Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged))
                            .then(|| chosen_path.clone());
                    // A17 T4 (F4 Error table): captured BEFORE the match below moves `outcome`'s
                    // Err payload — a genuine SaveError (IO/symlink) must land Sticky/Error so it
                    // survives the next keystroke, unlike the ordinary Info completion messages.
                    let is_save_error = outcome.is_err();
                    let mut status = String::new();
                    // Hoisted out of the `by_id_mut` block (local-then-assign, mirrors
                    // `status`/`fire_save`): the session-migration queue push below needs
                    // `editor` mutably, which conflicts with a live `&mut Buffer` borrow.
                    let mut migrate_from: Option<PathBuf> = None;
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        match outcome {
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                // MERGE-TIME capture. The dispatch-time `prior_key` is stale
                                // for a second Save-As dispatched before this merge landed;
                                // reading the buffer here gives the truth at THIS moment, so
                                // a->b then a->c records (a,b) then (b,c) and chains.
                                let pre_rekey = b.document.path.clone();
                                // Middle B: the buffer is rekeyed to the CHOSEN path so
                                // display, prefills, the open-dir seed, export derivation,
                                // wc.path(), and the LSP uri all stay logical.
                                if matches!(mode, SaveMode::SaveAs) {
                                    b.document.path = Some(chosen_path.clone());
                                    migrate_from = pre_rekey;
                                }
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
                    // Queue the session-entry migration. Nothing is queued when there is no
                    // old entry (first Save-As of an unnamed buffer) or when the path did not
                    // change (Save-As onto the same path).
                    if matches!(mode, SaveMode::SaveAs) {
                        if let Some(from) = migrate_from {
                            if from != chosen_path {
                                editor.pending_session_migrations.push_back(
                                    crate::editor::SessionMigration { from, to: chosen_path.clone() });
                            }
                        }
                    }
                    // Reconstruct the IDENTICAL Save(buffer_id, v) topic captured at the start so
                    // this completion collapses exactly its own "Saving…" lineage (§4.2). `buffer_id`
                    // and `v` are the same values the JobResult carries as `r.buffer_id`/`r.version`.
                    let topic = crate::status::StatusTopic::Save(buffer_id, v);
                    if is_save_error {
                        editor.finish_topic(topic, crate::status::StatusKind::Error, status);
                    } else {
                        editor.finish_topic(topic, crate::status::StatusKind::Info, status);
                    }
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
    // A plain Save resolves its own destination too — the document's path can itself be a
    // symlink (that is §4.10: openable but unsaveable).
    let resolved = match crate::fsx::resolve_write_destination(&*ctx.fs, &path) {
        Ok(r) => r,
        Err(crate::fsx::DestError::BrokenSymlink) => {
            ctx.editor.set_status_full(crate::status::StatusKind::Warning,
                format!("{}: destination symlink cannot be resolved", path.display()),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    do_save_to(ctx, SaveTarget { chosen: path, resolved }, SaveMode::Normal);
}

/// Registry `"save"` handler — unchanged public shape.
pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult {
    dispatch_save_reporting(ctx);
    CommandResult::Handled
}

/// The same work, RETURNING whether it opened a Save-As destination picker.
///
/// This return value is what replaces `dispatch_save_then`'s old
/// `minibuffer.kind == SaveAs` sniff. Inferring control flow from which overlay happens to
/// be up is what made that coupling silently breakable; the fact is now produced by the
/// function that knows it.
fn dispatch_save_reporting(ctx: &mut Ctx) -> bool {
    let path = match &ctx.editor.active().document.path {
        None => {
            let opened = crate::prompts::open_save_as(ctx.editor, &ctx.fs, &ctx.msg_tx);
            return opened;
        }
        Some(p) => p.clone(),
    };
    // External-mod check (§4.3 step 2): cheap stat; if the on-disk fingerprint
    // diverged from what we last wrote, refuse and raise the external-mod modal.
    let current_fp = fingerprint_with_fs(&*ctx.fs, &path);
    if current_fp != ctx.editor.active().document.stored_fp {
        ctx.editor.open_prompt(crate::prompt::Prompt::external_mod());
        ctx.editor.set_status_full(crate::status::StatusKind::Warning,
            "File changed on disk \u{2014} choose [R]eload or [O]verwrite",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return false;
    }
    do_save(ctx);
    false
}

/// The unified "save, then do `action`" entry. Goes through `dispatch_save_reporting`
/// (external-mod-checked). Handles all three buffer states:
/// - NAMED, no conflict → a save job is dispatched → arm `pending_after_save{action}`.
/// - NAMED, external-mod conflict → the modal was raised → do NOT arm (the user resolves
///   the modal and re-issues).
/// - UNNAMED → the Save-As destination picker opened → carry the action in
///   `pending_save_as` so it fires after the Save-As write completes. Gated on the RETURN
///   VALUE, not on inspecting which overlay is up — see the module-level hazard note.
pub(crate) fn dispatch_save_then(ctx: &mut crate::registry::Ctx, action: crate::editor::PostSaveAction) {
    let was_unnamed = ctx.editor.active().document.path.is_none();
    let buffer_id = ctx.editor.active().id;
    let v = ctx.editor.active().document.version;
    let opened_save_as = dispatch_save_reporting(ctx);
    if was_unnamed {
        if opened_save_as {
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
        ctx.editor.set_status_full(crate::status::StatusKind::Warning, "No file name — use Save As".to_string(),
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
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
        Err(e) => {
            editor.set_status_full(crate::status::StatusKind::Error, e.to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
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
    // A17 T8 category (b): route the wholesale swap through the single chokepoint. On a read-only
    // buffer it no-ops + sets the Sticky Warning and returns false — the epilogue and the "Reloaded"
    // ack below are skipped, so no false success is reported.
    if !editor.replace_buffer(editor.active, crate::editor::Buffer { id, ..new_buf }) { return; }
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
    editor.set_status(crate::status::StatusKind::Info, "Reloaded");
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
    // A17 T8 category (b): route through the single chokepoint (see reload_from_disk).
    if !editor.replace_buffer(editor.active, crate::editor::Buffer { id, ..new_buf }) { return; }
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
    editor.set_status(crate::status::StatusKind::Info, "Recovered unsaved changes");
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

    // A17 T8 — CATEGORY (b): whole-buffer REPLACEMENT (reload) on a read-only buffer is refused,
    // NOT replaced, and reports "buffer is read-only" — never a false "Reloaded".
    #[test]
    fn reload_on_read_only_buffer_is_refused_not_replaced() {
        let path = scratch();
        std::fs::write(&path, "disk contents\n").unwrap();
        let mut e = Editor::new_from_text("buffer contents\n", Some(path.clone()), (80, 24));
        let before = e.active().document.buffer.to_string();
        e.active_mut().read_only = true;
        crate::save::reload_from_disk(&mut e);
        assert_eq!(e.active().document.buffer.to_string(), before, "read-only buffer NOT replaced by reload");
        assert_eq!(e.status_text(), "buffer is read-only");
        assert_ne!(e.status_text(), "Reloaded", "must NOT report a false Reloaded");
        let _ = std::fs::remove_file(&path);
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
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() };
            dispatch_save(&mut ctx);
        }
        assert_eq!(e.status_text(), "Saving\u{2026}", "status set before dispatch (§3.9)");
        // InlineExecutor already ran the job; apply the buffered merge.
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty(), "version==saved_version after save → clean");
        assert_eq!(e.status_text(), "Saved");
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        // User edits on to version 2 BEFORE the merge applies.
        e.active_mut().document.version = 2;
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        // saved_version recorded v1, but the buffer is at v2 → still dirty.
        assert_eq!(e.active().document.saved_version, Some(1));
        assert!(e.active().document.dirty(), "edited-on buffer stays dirty after a stale-version save");
        let _ = std::fs::remove_file(&p);
    }

    // Superseded by Task 15/16 (§7.6.1/§4.10): `do_save` now resolves a symlinked document
    // path through `resolve_write_destination` BEFORE dispatch, exactly like a Save-As
    // destination, so a plain Save whose own `Document.path` is a symlink no longer fails —
    // it succeeds, writing to the resolved target while the link itself survives. This
    // replaces the old "save through a symlink is refused" premise, which was the defect
    // this task fixes; the still-failing "last resort" guard inside `save_atomic` itself
    // remains covered directly by `file::tests::save_through_symlink_refused`.
    #[cfg(unix)]
    #[test]
    fn background_save_through_symlinked_document_path_resolves_and_succeeds() {
        let real = scratch();
        let link = scratch();
        std::fs::write(&real, "real\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut e = Editor::new_from_text("x\n", Some(link.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty(), "a resolved symlink destination saves cleanly");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "x\n", "bytes land on the RESOLVED target");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink(), "the link itself survives");
        assert_eq!(e.active().document.path.as_deref(), Some(link.as_path()),
            "Document.path stays the CHOSEN (logical) path, not the resolved one");
        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
    }

    /// A17 T4: a genuine background-save failure (SaveError, via the same ENOTDIR trigger as
    /// `background_save_failure_parent_is_file_keeps_dirty`) must land as a Sticky Error — it
    /// must survive a later Info ack rather than silently clearing on the next keystroke (Q1).
    /// (Was triggered via a symlink refusal; Task 15/16 made symlinked destinations resolve
    /// and succeed instead, so the failure trigger moved to a real I/O error.)
    #[cfg(unix)]
    #[test]
    fn background_save_failure_is_a_sticky_error_that_survives_a_later_info() {
        let parent = scratch();
        std::fs::write(&parent, "i am a file, not a dir\n").unwrap();
        let target = parent.join("doc.md"); // ENOTDIR on temp create

        let mut e = Editor::new_from_text("x\n", Some(target.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        // Q1: an Info does NOT displace a held Error.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        let _ = std::fs::remove_file(&parent);
    }

    /// A successful background save is still an ordinary Info/Transient status (the failure
    /// branch above must not have made ALL completions Sticky-Error).
    #[test]
    fn background_save_success_is_still_transient_info() {
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Info);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Transient);
        let _ = std::fs::remove_file(&p);
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert!(e.active().document.dirty(), "failed save must leave the buffer dirty");
        assert!(e.active().document.saved_version.is_none());
        // Assert the ENOTDIR error itself surfaced (not merely any non-empty status) — this
        // proves the save was attempted and failed, NOT that the external-mod modal opened.
        assert!(
            e.status_text().to_lowercase().contains("not a directory") || e.status_text().contains("ENOTDIR"),
            "OS ENOTDIR must surface as status; got: {:?}", e.status_text()
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty());
        assert!(!sp.exists(), "a save that leaves the buffer clean deletes the swap");

        // Now: dispatch a save at v2, but edit on to v3 before the merge → keep swap.
        crate::swap::write_atomic(&sp, "stub2").unwrap();
        e.active_mut().document.version = 2;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "external change must raise the modal, not clobber");
        assert!(ex.drain().is_empty(), "no save job dispatched on conflict");
        // A17 T5 (F4 Warning table): the external-mod refusal is a Sticky Warning.
        assert_eq!(e.status_text(), "File changed on disk \u{2014} choose [R]eload or [O]verwrite");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
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
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() };
          dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "a file appearing where there was none is a conflict");
        assert!(ex.drain().is_empty(), "no save job dispatched on new-file conflict");
        let _ = std::fs::remove_file(&p);
    }

    /// A17 T5 (F4 Warning table): an `overwrite_save` on a pathless (unnamed) buffer refuses
    /// with a Sticky Warning, not an ordinary Info echo.
    #[test]
    fn overwrite_save_on_unnamed_buffer_is_a_sticky_warning() {
        use crate::jobs::InlineExecutor;
        use crate::registry::Ctx;
        let mut e = Editor::new_from_text("mine\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; overwrite_save(&mut ctx); }
        assert_eq!(e.status_text(), "No file name — use Save As");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(ex.drain().is_empty(), "no save job dispatched on the refusal");
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() }; overwrite_save(&mut ctx); }
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
    fn reload_from_disk_failure_is_a_sticky_error_that_survives_a_later_info() {
        // A path that will fail to read → the reload Err arm fires.
        let missing = std::path::PathBuf::from("/nonexistent/definitely/missing-a17.md");
        let mut e = Editor::new_from_text("hello\n", Some(missing), (80, 24));
        crate::save::reload_from_disk(&mut e);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        // Q1: an Info does NOT displace a held Error.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
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
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() };
            dispatch_save(&mut ctx);
        }
        assert!(ex.drain().is_empty(), "no save job dispatched on external-mod conflict");
        assert!(e.status_text().to_lowercase().contains("changed on disk"), "status surfaces the refusal");
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
        crate::test_support::install_enabled_harper(&mut e); // enable Harper so late results reach the version gate
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
        // The stale result must NOT be stored — the non-creating latch-clear leaves no phantom slot.
        assert!(e.active().diagnostics.slot(harper).is_none(),
            "late pre-reload DiagnosticsDone must be discarded (version gate); no phantom slot");
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
        crate::test_support::install_enabled_harper(&mut e); // enable Harper so the late result reaches the version gate
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
        assert!(e.active().diagnostics.slot(harper).is_none(),
            "late pre-recovery DiagnosticsDone must be discarded; no phantom slot");
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
        let fp = fingerprint_with_limit(&crate::fsx::RealFs, &p, 4)
            .expect("over-cap present file still yields a fingerprint");
        assert_eq!(fp.size, 10, "size from metadata");
        assert_eq!(fp.hash, 0, "over-cap → sentinel hash, NOT None (closes None==None)");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fingerprint_within_cap_hashes_content_unchanged() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").unwrap();
        let within = fingerprint_with_limit(&crate::fsx::RealFs, &p, 1_000_000).expect("fp");
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

    #[cfg(unix)]
    #[test]
    fn fingerprint_on_a_broken_symlink_is_none() {
        // BEHAVIOUR PRESERVATION. Today `fingerprint` opens with
        // `std::fs::metadata(path).ok()?`, so a broken symlink yields None. Under the seam,
        // `stat` SUCCEEDS for a broken link (broken == true) — so the caller must map
        // broken -> None explicitly. Without that mapping this returns Some with zeroed
        // fields and silently corrupts the external-mod comparison.
        let d = std::env::temp_dir().join(format!("wc-fp-broken-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let link = d.join("dangling.md");
        std::os::unix::fs::symlink(d.join("gone.md"), &link).expect("symlink");
        assert!(fingerprint_with_fs(&crate::fsx::RealFs, &link).is_none(),
            "a broken symlink must fingerprint as None, exactly as metadata().ok()? did");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fingerprint_faults_are_injectable() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").expect("seed");
        let ff = crate::test_support::FaultFs::new(crate::test_support::FaultAt::Stat);
        assert!(fingerprint_with_fs(&ff, &p).is_none(),
            "an injected stat failure degrades to None, matching today's .ok()?");
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
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(), fs: crate::test_support::test_fs() };
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
        assert!(e.status_text().to_lowercase().contains("save"));
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn save_as_onto_a_symlink_splits_chosen_and_resolved_correctly() {
        // THE highest-value test of this task: one SaveTarget field going to the wrong
        // consumer reintroduces either §4.10's defect (unsaveable symlinks) or §7.6.2's
        // seven regressions (canonical Document.path). All five consumers asserted at once.
        let real = scratch();
        let link = scratch();
        std::fs::write(&real, b"original\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let mut e = Editor::new_from_text("new body\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let resolved = std::fs::canonicalize(&link).expect("canonicalize");
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(crate::fsx::RealFs),
            };
            do_save_to(&mut ctx,
                SaveTarget { chosen: link.clone(), resolved: resolved.clone() },
                SaveMode::SaveAs);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // 1. The write landed on the RESOLVED target…
        assert_eq!(std::fs::read_to_string(&real).expect("read target"), "new body\n");
        // 2. …and the link survived as a link.
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "the symlink must survive — atomic_replace renamed over the TARGET");
        // 3. Document.path holds the CHOSEN path (Middle B).
        assert_eq!(e.active().document.path.as_deref(), Some(link.as_path()),
            "the buffer keeps the path the writer chose, not the canonical target");
        // 4. stored_fp describes the written file, so a follow-up save sees no conflict.
        assert_eq!(e.active().document.stored_fp, crate::save::fingerprint(&resolved),
            "stored_fp must match the file actually written");
        assert!(!e.active().document.dirty(), "and the buffer is clean");

        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
    }

    #[test]
    fn an_injected_fault_reaches_the_save_worker() {
        // The seam extension's headline payoff, asserted. The worker-side write is
        // fault-testable for the FIRST time — it previously hardcoded RealFs inside the job
        // closure. This FAILS if an implementer calls `file::save_atomic` (the RealFs
        // wrapper) instead of `save_atomic_with_fs` with the Arc cloned from Ctx.fs.
        //
        // FAIL-VERIFY: call `file::save_atomic` here, watch this fail (the file is written
        // for real and the buffer goes clean), then revert.
        let p = scratch();
        std::fs::write(&p, b"old\n").expect("seed");
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(
                    crate::test_support::FaultFs::new(crate::test_support::FaultAt::Rename)),
            };
            do_save_to(&mut ctx, SaveTarget::same(p.clone()), SaveMode::Normal);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.active().document.dirty(),
            "an injected write failure must leave the buffer dirty — if this passes as clean, \
             the worker bypassed the seam and wrote for real");
        assert_eq!(std::fs::read_to_string(&p).expect("read"), "old\n",
            "and the file on disk is untouched");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_target_same_sets_both_fields() {
        let p = std::path::PathBuf::from("/tmp/x.md");
        let t = SaveTarget::same(p.clone());
        assert_eq!(t.chosen, p);
        assert_eq!(t.resolved, p, "the common case: no resolution needed, both equal");
    }

    // -----------------------------------------------------------------------
    // Task 21 — the quit-drain coupling hazard
    // -----------------------------------------------------------------------

    #[test]
    fn save_and_quit_on_an_unnamed_buffer_completes_through_the_picker() {
        // THE HAZARD, asserted. `dispatch_save_then` armed `pending_save_as` by checking
        // `minibuffer.kind == SaveAs`. Once Save-As opens a PICKER, that check is false
        // forever — no compile error, no visible bug in the common path, but save-and-quit
        // on an unnamed buffer silently stops completing.
        let mut e = Editor::new_from_text("unsaved\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(crate::fsx::RealFs),
            };
            dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::Quit);
        }
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()),
            "an unnamed buffer opens the DESTINATION picker, not a minibuffer");
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::Quit),
            "and the post-save action is armed — this is what the minibuffer sniff used to do");
    }

    #[test]
    fn esc_out_of_a_drain_destination_picker_aborts_the_drain() {
        // The Effort-6 Codex-C2 fix, carried to the new path. Without it, backing out leaves
        // quit_drain Some-but-inert: stranded with no in-flight save and nothing to re-drive.
        let mut e = Editor::new_from_text("unsaved\n", None, (80, 24));
        e.quit_drain = Some(crate::editor::QuitDrain {
            queue: std::collections::VecDeque::new(),
            mode: crate::editor::QuitMode::SaveAll });
        e.pending_save_as = Some(crate::editor::PostSaveAction::ContinueQuitDrain);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), String::new());

        // Driven through the REAL intercept. Calling `cancel_destination` directly is the
        // pattern this very task's commit-arm comment condemns: Task 18 ships a plain
        // `editor.file_browser = None` Esc arm that THIS task must replace, and a direct call
        // passes whether or not that replacement happened.
        //
        // FAIL-VERIFY: leave Task 18's plain Esc arm in place, watch the drain assertions fail.
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Esc);

        assert!(e.file_browser.is_none(), "Esc closes the picker");
        assert!(e.pending_save_as.is_none(), "and clears the armed action");
        assert!(e.quit_drain.is_none(), "and ABORTS the drain rather than stranding it");
        assert!(!e.quit_drain_advance);
        assert!(!e.quit, "backing out must not quit");
    }
}
