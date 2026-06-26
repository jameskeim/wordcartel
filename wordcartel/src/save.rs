//! Background save (spec §4.3). The foreground captures an O(1) rope snapshot +
//! version + path and dispatches a JobKind::Save job; the worker materializes
//! the snapshot off the keystroke path and atomically writes it; the merge
//! updates status/saved_version version-awarely.

use std::path::Path;
use std::time::SystemTime;

use crate::commands::CommandResult;
use crate::file::{self, SaveOutcome};
use crate::jobs::{Job, JobKind, JobResult, ResultClass};
use crate::registry::Ctx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileFingerprint {
    pub mtime: Option<SystemTime>,
    pub size: u64,
}

/// Fingerprint a path, or `None` if it does not exist / cannot be stat'd.
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileFingerprint { mtime: meta.modified().ok(), size: meta.len() })
}

/// Internal: dispatch the save job (no external-mod check). Called by
/// `dispatch_save` (after the check) and `overwrite_save` (bypassing it).
fn do_save(ctx: &mut Ctx) {
    let path = ctx.editor.active().document.path.clone().expect("do_save called without a path");

    // §3.9: status BEFORE dispatch. O(1) snapshot; version captured now.
    ctx.editor.status = "Saving\u{2026}".to_string();
    let snap = ctx.editor.active().document.buffer.snapshot(); // O(1) ropey clone
    let v = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;

    ctx.executor.dispatch(Job {
        buffer_id,
        class: ResultClass::Durability,
        version: v,
        kind: JobKind::Save,
        run: Box::new(move || {
            // Worker: materialize the snapshot off the keystroke path, then write.
            let content = snap.to_string();
            let outcome = file::save_atomic(&path, &content);
            let new_fp = fingerprint(&path);
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
                    let mut status = String::new();
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        match outcome {
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                b.document.saved_version = Some(v);
                                b.document.stored_fp = new_fp;
                                if b.document.version == v {
                                    status = "Saved".to_string();
                                    crate::swap::delete(b.document.path.as_deref());
                                } else {
                                    status = format!("Saved v{v} (still editing)");
                                }
                            }
                            Err(e) => {
                                // Failure: leave saved_version/stored_fp untouched
                                // (buffer stays dirty); surface the error.
                                // Do NOT re-introduce SaveError::Symlink match — e.to_string()
                                // for Symlink still contains "symlink", satisfying the test.
                                status = e.to_string();
                            }
                        }
                    }
                    editor.status = status;
                }),
            }
        }),
    });
}

/// Registry `"save"` handler: external-mod check then dispatch a background save.
pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult {
    let path = match &ctx.editor.active().document.path {
        None => {
            ctx.editor.status = "No file name (save-as is Effort 5)".to_string();
            return CommandResult::Handled;
        }
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

/// Save bypassing the fingerprint conflict (the [O]verwrite modal action).
pub fn overwrite_save(ctx: &mut Ctx) {
    if ctx.editor.active().document.path.is_none() {
        ctx.editor.status = "No file name (save-as is Effort 5)".to_string();
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
    let id = editor.active().id;                 // preserve THIS buffer's id
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    // Clear any stale search/diag overlay — the buffer content has changed wholesale.
    editor.search = None;
    editor.diag = None;
    // then the existing follow-ups, now on the active buffer:
    editor.active_mut().view.line_layouts.clear();
    crate::derive::rebuild(editor);
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
    let id = editor.active().id;                 // preserve THIS buffer's id
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    // Clear any stale search/diag overlay — the buffer content has changed wholesale.
    editor.search = None;
    editor.diag = None;
    editor.active_mut().view.line_layouts.clear();
    crate::derive::rebuild(editor);
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
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
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
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
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
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.active().document.dirty(), "failed save must leave the buffer dirty");
        assert!(e.active().document.saved_version.is_none());
        assert!(e.status.to_lowercase().contains("symlink"));
        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
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
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(!e.active().document.dirty());
        assert!(!sp.exists(), "a save that leaves the buffer clean deletes the swap");

        // Now: dispatch a save at v2, but edit on to v3 before the merge → keep swap.
        crate::swap::write_atomic(&sp, "stub2").unwrap();
        e.active_mut().document.version = 2;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() }; dispatch_save(&mut ctx); }
        e.active_mut().document.version = 3; // edited on
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
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
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
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
        e.active_mut().diagnostics.in_flight_version = Some(pre_reload_version);
        e.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "fake".into(),
                suggestions: vec![],
            }
        ];
        e.active_mut().diagnostics.computed_version = pre_reload_version;
        reload_from_disk(&mut e);
        // Post-reload: version must be strictly greater than pre_reload_version.
        assert!(e.active().document.version > pre_reload_version,
            "reload must bump version past the pre-reload value");
        // DiagStore must be cleared of any stale underlines.
        assert!(e.active().diagnostics.diagnostics.is_empty(),
            "reload must reset DiagStore (no stale underlines)");
        assert!(e.active().diagnostics.in_flight_version.is_none(),
            "reload must clear in_flight_version");
        // Now deliver the late DiagnosticsDone for the pre-reload version V.
        let new_version = e.active().document.version;
        let buffer_id = e.active().id;
        crate::diagnostics_run::apply_diagnostics_done(
            &mut e,
            buffer_id,
            pre_reload_version, // stale version
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "stale".into(),
                suggestions: vec![],
            }],
        );
        // The stale result must NOT be stored.
        assert!(e.active().diagnostics.diagnostics.is_empty(),
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
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "fresh".into(),
                suggestions: vec![],
            }],
        );
        assert_eq!(e.active().diagnostics.diagnostics.len(), 1,
            "fresh result for the new version must be stored");
        let _ = std::fs::remove_file(&p);
    }

    /// Same invariant for load_recovered: a late DiagnosticsDone for the
    /// pre-recovery version is discarded after the version bump.
    #[test]
    fn load_recovered_discards_pre_recovery_diagnostics_done() {
        let mut e = Editor::new_from_text("old content\n", None, (80, 24));
        let pre_recovery_version = e.active().document.version; // 0
        e.active_mut().diagnostics.in_flight_version = Some(pre_recovery_version);
        // Simulate stale underlines that must be wiped by recovery.
        e.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "old".into(),
                suggestions: vec![],
            }
        ];
        e.active_mut().diagnostics.computed_version = pre_recovery_version;
        load_recovered(&mut e, "recovered content\n");
        assert!(e.active().document.version > pre_recovery_version,
            "load_recovered must bump version past the pre-recovery value");
        assert!(e.active().diagnostics.diagnostics.is_empty(),
            "load_recovered must reset DiagStore");
        assert!(e.active().diagnostics.in_flight_version.is_none(),
            "load_recovered must clear in_flight_version");
        let buffer_id = e.active().id;
        // Deliver a late result for the pre-recovery version — must be discarded.
        crate::diagnostics_run::apply_diagnostics_done(
            &mut e,
            buffer_id,
            pre_recovery_version,
            vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "stale".into(),
                suggestions: vec![],
            }],
        );
        assert!(e.active().diagnostics.diagnostics.is_empty(),
            "late pre-recovery DiagnosticsDone must be discarded");
    }
}
