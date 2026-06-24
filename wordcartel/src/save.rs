//! Background save (spec §4.3). The foreground captures an O(1) rope snapshot +
//! version + path and dispatches a JobKind::Save job; the worker materializes
//! the snapshot off the keystroke path and atomically writes it; the merge
//! updates status/saved_version version-awarely.

use std::path::Path;
use std::time::SystemTime;

use crate::commands::CommandResult;
use crate::file::{self, SaveOutcome};
use crate::jobs::{Job, JobKind, JobResult};
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

    ctx.executor.dispatch(Job {
        version: v,
        kind: JobKind::Save,
        run: Box::new(move || {
            // Worker: materialize the snapshot off the keystroke path, then write.
            let content = snap.to_string();
            let outcome = file::save_atomic(&path, &content);
            let new_fp = fingerprint(&path);
            JobResult {
                version: v,
                kind: JobKind::Save,
                merge: Box::new(move |editor| {
                    match outcome {
                        Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                            // Task 2: route by buffer_id
                            editor.active_mut().document.saved_version = Some(v);
                            editor.active_mut().document.stored_fp = new_fp;
                            if editor.active().document.version == v {
                                editor.status = "Saved".to_string();
                                // Buffer is clean at the saved version → swap is
                                // no longer needed. (Stale-version saves skip this.)
                                crate::swap::delete(editor.active().document.path.as_deref());
                            } else {
                                editor.status = format!("Saved v{v} (still editing)");
                            }
                        }
                        Err(e) => {
                            // Failure: leave saved_version/stored_fp untouched
                            // (buffer stays dirty); surface the error.
                            editor.status = e.to_string();
                        }
                    }
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
        ctx.editor.prompt = Some(crate::prompt::Prompt::external_mod());
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
    let area = editor.active().view.area;
    let fresh = crate::editor::Editor::new_from_text(&text, Some(path.clone()), area); // saved_version=Some(0) → clean
    let new_buf = fresh.buffers.into_iter().next().expect("new_from_text yields one buffer");
    let id = editor.active().id;                 // preserve THIS buffer's id
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
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
    let area = editor.active().view.area;
    let fresh = crate::editor::Editor::new_from_text(body, path.clone(), area);
    let new_buf = fresh.buffers.into_iter().next().expect("new_from_text yields one buffer");
    let id = editor.active().id;                 // preserve THIS buffer's id
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    editor.active_mut().document.saved_version = None; // recovered work is unsaved
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
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(!e.active().document.dirty());
        assert!(!sp.exists(), "a save that leaves the buffer clean deletes the swap");

        // Now: dispatch a save at v2, but edit on to v3 before the merge → keep swap.
        crate::swap::write_atomic(&sp, "stub2").unwrap();
        e.active_mut().document.version = 2;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
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
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex };
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; overwrite_save(&mut ctx); }
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
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
            dispatch_save(&mut ctx);
        }
        assert!(ex.drain().is_empty(), "no save job dispatched on external-mod conflict");
        assert!(e.status.to_lowercase().contains("changed on disk"), "status surfaces the refusal");
        assert!(e.active().document.dirty(), "buffer stays dirty when a save is refused");
        let _ = std::fs::remove_file(&p);
    }
}
