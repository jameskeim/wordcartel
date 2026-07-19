//! Session/resume restoration: cursor/scroll/marks/folds/block restore and the
//! open-into-current buffer-load seam. Extracted verbatim from app.rs (Effort H1).

use crate::editor::Editor;

/// Decide the resume position: restore (cursor clamped to doc_len) only if the
/// stored mtime+size identity matches the current file. Mismatch → None (stale).
pub fn apply_resume(
    e: &crate::state::StateEntry,
    current: (i64, u64),
    doc_len: usize,
) -> Option<(usize, usize)> {
    if (e.mtime, e.size) != current {
        return None;
    }
    Some((e.cursor.min(doc_len), e.scroll))
}

/// Populate the active buffer's marks from a session entry (string→char keys),
/// clamped+grapheme-snapped. Call only when the staleness guard has accepted
/// the entry (mirrors cursor/scroll restore).
pub fn load_marks_from_entry(editor: &mut Editor, entry: &crate::state::StateEntry) {
    for (k, &raw) in &entry.marks {
        if let Some(ch) = k.chars().next() {
            let off = crate::nav::clamp_snap(editor, raw);
            editor.active_mut().marks.insert(ch, off);
        }
    }
}

/// Restore the persisted marked block from a session entry into the active buffer.
/// Call only when the staleness guard has accepted the entry (mirrors cursor/marks).
///
/// Both endpoints are clamped+grapheme-snapped via `clamp_snap` (the SAME treatment
/// marks get). This is load-bearing: `persist_session` records the block from the
/// in-memory buffer, but the staleness guard keys on on-disk mtime+size. A dirty
/// buffer longer than the on-disk file can persist a block whose `end` exceeds the
/// on-disk length; on a dirty-quit + reopen-unchanged-file cycle the buffer reloads
/// SHORTER yet the guard still passes, so a raw restore would hand `block_*` ops an
/// out-of-range range and `buffer.slice()` would assert/panic. Clamping prevents that;
/// a block that collapses to empty after clamping is dropped.
pub fn load_block_from_entry(editor: &mut Editor, entry: &crate::state::StateEntry) {
    if let Some((s, en)) = entry.block {
        let s = crate::nav::clamp_snap(editor, s);
        let en = crate::nav::clamp_snap(editor, en);
        if s < en {
            editor.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: s, end: en, hidden: false });
        }
    }
}

/// Restore session-resume state (cursor, scroll, marks, folds) for `path` into the
/// active buffer. Factored verbatim from run()'s launch resume block so launch and
/// `open_into_current` share one code path. Reloads `state::load()` itself so it works
/// with only `&mut Editor`. No-op if there is no matching/non-stale session entry.
pub fn restore_resume(editor: &mut Editor, path: &std::path::Path) {
    let session = crate::state::load();
    // fs-chokepoint-allow: (c) pure path resolution — a name computation, not content access
    if let Ok(canon) = std::fs::canonicalize(path) {
        let key = canon.to_string_lossy().into_owned();
        if let Some(entry) = session.entries.get(&key) {
            if let Some(identity) = crate::state::file_identity(path) {
                let doc_len = editor.active().document.buffer.len();
                if let Some((cur, scroll)) = apply_resume(entry, identity, doc_len) {
                    let sel = wordcartel_core::selection::Selection::single(cur);
                    editor.active_mut().document.selection = sel;
                    editor.active_mut().view.scroll = scroll;
                    load_marks_from_entry(editor, entry);
                    editor.active_mut().folds.replace_folded(entry.folds.iter().copied().collect());
                    let (blocks, buf) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.clone()) };
                    editor.active_mut().folds.reconcile(&blocks, &buf);
                    load_block_from_entry(editor, entry);
                }
            }
        }
    }
}

/// Effort 6: load persisted scratch content into the scratch buffer. Replaces the
/// scratch Buffer in place (fresh id so any stale job no-ops), then clamp-snaps the
/// cursor into `[0, len]` on a char boundary (mirrors 9A's clamp discipline so a
/// stale offset never panics a later `slice()`). No-op if no scratch installed.
pub fn restore_scratch(editor: &mut Editor, st: &crate::state::ScratchState) {
    let Some(sid) = editor.scratch_id else { return; };
    let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) else { return; };
    let area = editor.buffers[idx].view.area;
    let id = editor.alloc_id();
    // A17 T8 category (b): route the wholesale swap through the single chokepoint. The scratch slot
    // is never read-only, so this always succeeds; guarded for closure completeness.
    if !editor.replace_buffer(idx, crate::editor::Buffer::from_text(id, &st.text, None, area)) { return; }
    editor.scratch_id = Some(id);
    // Update MRU id mapping (old scratch id → new).
    for m in editor.mru.iter_mut() { if *m == sid { *m = id; } }
    // Char-boundary clamp via `nav::clamp_snap` (Codex I3: TextBuffer has NO
    // `snap_to_boundary`; clamp_snap at nav.rs:164 operates on the ACTIVE buffer).
    // restore_scratch runs at startup; briefly make scratch active to clamp, then
    // restore the prior active index.
    let prev_active = editor.active;
    editor.active = idx;
    let cur = crate::nav::clamp_snap(editor, st.cursor);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(cur);
    editor.active = prev_active;
}

/// Open `path` into the active buffer slot (the buffer-load seam reused by Tasks 2/4/5).
/// Allocates a FRESH id so an in-flight save/swap job for the replaced buffer merges via
/// `by_id_mut(old_id)` → `None` (harmless no-op). On OpenError: set status, do NOT replace
/// (keep the user's work).
pub fn open_into_current(editor: &mut Editor, fs: &dyn crate::fsx::Fs, path: &std::path::Path) {
    let old_id = editor.active().id; // capture BEFORE alloc so MRU can replace old→new
    let id = editor.alloc_id(); // FRESH id → an in-flight job for the old buffer no-ops via by_id_mut(old_id)=None
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, fs, path, area) {
        Ok(b) => {
            let a = editor.active;
            // A17 T8 category (b): route through the single chokepoint. On a read-only buffer this
            // no-ops + Sticky Warning and returns false — the MRU/rebuild/fire-event epilogue below
            // is skipped and the user's read-only view is preserved.
            if !editor.replace_buffer(a, b) { return; }
            // Keep MRU consistent: remove the ghost old id and put the new id at front.
            // Mirrors the close_buffer / restore_scratch patterns (workspace.rs, app.rs).
            editor.mru.retain(|&x| x != old_id);
            editor.touch_mru(id);
            if editor.resume_enabled {
                restore_resume(editor, path);
            }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.clear_status();
            crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::Open, Some(path));
        }
        Err(e) => {
            editor.set_status_full(crate::status::StatusKind::Error, e.to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
}

/// Record the active buffer's position into the session store and flush to disk.
/// Scratch content is always captured; per-file entry only for named buffers.
/// A write failure → status warning only (never blocks quit or loses the document).
pub(crate) fn persist_session(
    session: &mut crate::state::SessionState,
    editor: &crate::editor::Editor,
    cfg: &crate::config::Config,
    seq: u64,
) {
    // Effort 6: capture scratch content first, independent of the active buffer.
    // M5: guard on byte length — never materialize a huge String for persistence.
    if let Some(sid) = editor.scratch_id {
        if let Some(sb) = editor.by_id(sid) {
            if sb.document.buffer.len() <= crate::limits::MAX_SESSION_BYTES {
                session.scratch = Some(crate::state::ScratchState {
                    text: sb.document.buffer.to_string(),
                    cursor: sb.document.selection.primary().head,
                });
            } else {
                // Oversized: skip persisting the live scratch — and CLEAR any stale scratch
                // loaded from disk, so an old session's scratch is not resurrected. The live
                // buffer is untouched; only its cross-session persistence is dropped.
                session.scratch = None;
            }
        }
    }
    // Per-file entry for the active buffer (unchanged): only when it has a real,
    // canonicalizable path. Scratch/new buffers contribute no per-file entry.
    if let Some(raw_path) = editor.active().document.path.as_deref() {
        // fs-chokepoint-allow: (c) pure path resolution — a name computation, not content access
        if let Ok(canon) = std::fs::canonicalize(raw_path) {
            if let Some((mtime, size)) = crate::state::file_identity(raw_path) {
                let entry = crate::state::StateEntry {
                    cursor: editor.active().document.selection.primary().head,
                    scroll: editor.active().view.scroll,
                    marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
                    mtime, size, seq,
                    folds: editor.active().folds.folded().iter().copied().collect(),
                    block: editor.active().marked_block.map(|b| (b.start, b.end)),
                    id: Some(editor.active().document.id.to_hex()),
                };
                session.record(canon.to_string_lossy().into_owned(), entry, cfg.state.max_entries);
            }
        }
    }
    // Always flush — scratch durability does not depend on the active buffer.
    let _ = session.save();
}

#[cfg(test)]
pub fn persist_session_for_test(s: &mut crate::state::SessionState, e: &crate::editor::Editor, cfg: &crate::config::Config, seq: u64) {
    persist_session(s, e, cfg, seq);
}

/// Apply every queued Save-As session-entry migration, FIFO.
///
/// FIFO is required, not incidental: with merge-time capture each entry's `from` is the
/// previous entry's `to`, so any other order strands the chain.
///
/// Best-effort — this is hygiene, not a durability guarantee. A migration whose `from` key
/// is already absent is a silent no-op, never an error and never a reason to fail a persist.
pub(crate) fn drain_session_migrations(
    session: &mut crate::state::SessionState,
    editor: &mut crate::editor::Editor,
    cfg: &crate::config::Config,
) {
    while let Some(m) = editor.pending_session_migrations.pop_front() {
        // Both endpoints are LOGICAL paths (Middle B); canonicalizing here is what makes a
        // symlinked destination converge on the same key as its target.
        // fs-chokepoint-allow: (c) pure path resolution — a name computation, not content access
        let from_key = std::fs::canonicalize(&m.from)
            .unwrap_or_else(|_| m.from.clone()).to_string_lossy().into_owned();
        // fs-chokepoint-allow: (c) pure path resolution — a name computation, not content access
        let to_key = std::fs::canonicalize(&m.to)
            .unwrap_or_else(|_| m.to.clone()).to_string_lossy().into_owned();
        if from_key == to_key { continue; }
        let Some(mut entry) = session.entries.remove(&from_key) else { continue };
        entry.seq = session.next_seq();
        session.record(to_key, entry, cfg.state.max_entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestClock;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::registry::Ctx;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);
    struct Z; impl wordcartel_core::history::Clock for Z { fn now_ms(&self) -> u64 { 0 } }
    fn tx() -> std::sync::mpsc::Sender<crate::app::Msg> {
        std::sync::mpsc::channel().0
    }
    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wcartel-sessmig-{}-{}.md",
            std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
    }

    #[test]
    fn two_migrations_in_one_drain_batch_both_apply() {
        // FAIL-VERIFY: swap the queue for an `Option` slot, watch this fail, then revert.
        //
        // FAILS AGAINST AN `Option` SLOT. `app::fold_and_continue` drains the executor in a
        // LOOP (`for o in ex.drain() { apply_job_outcome(…) }`), so several ready save jobs
        // merge before app::run next reaches a persist point. A single slot would keep only
        // the last, silently losing the first writer's marks with no error.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let cfg = crate::config::Config::default();
        let mut s = crate::state::SessionState::default();
        let entry = |c: usize| crate::state::StateEntry {
            cursor: c, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq: 1,
            folds: vec![], block: None, ..Default::default() };
        s.entries.insert("/a.md".into(), entry(11));
        s.entries.insert("/x.md".into(), entry(22));

        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/a.md".into(), to: "/b.md".into() });
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/x.md".into(), to: "/y.md".into() });

        drain_session_migrations(&mut s, &mut e, &cfg);

        assert!(s.entries.contains_key("/b.md"), "first migration applied");
        assert!(s.entries.contains_key("/y.md"), "second migration applied — NOT clobbered");
        assert_eq!(s.entries["/b.md"].cursor, 11, "and it carried the cursor across");
        assert_eq!(s.entries["/y.md"].cursor, 22);
        assert!(!s.entries.contains_key("/a.md") && !s.entries.contains_key("/x.md"),
            "the old keys are gone — the point is to remove the stale duplicate");
        assert!(e.pending_session_migrations.is_empty(), "the queue drains fully");
    }

    #[test]
    fn overlapping_same_source_save_as_chains_correctly() {
        // DRIVES THE REAL DISPATCH. An earlier version hand-enqueued (a->b, b->c) — the
        // already-correct sequence — so it passed even if `do_save_to` never captured
        // anything. It guarded nothing, which is exactly what it was split out to prevent.
        //
        // FAIL-VERIFY: move the capture back to dispatch time (bind `prior_key` before the
        // job and use it for the migration), watch this fail with an entry stranded at /b.md.
        let p_a = scratch();
        let p_b = scratch();
        let p_c = scratch();
        std::fs::write(&p_a, b"body\n").expect("seed");
        let mut e = Editor::new_from_text("body\n", Some(p_a.clone()), (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);

        // TWO Save-As dispatched from the SAME source before either merge lands. This is the
        // ordering dispatch-time capture gets wrong: it would record (a->b, a->c).
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex,
                msg_tx: tx(), fs: std::sync::Arc::clone(&fs) };
            crate::save::do_save_to(&mut ctx,
                crate::save::SaveTarget::same(p_b.clone()), crate::save::SaveMode::SaveAs);
            crate::save::do_save_to(&mut ctx,
                crate::save::SaveTarget::same(p_c.clone()), crate::save::SaveMode::SaveAs);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // The queue the MERGES produced — not one we wrote by hand.
        let mut s = crate::state::SessionState::default();
        let key = |p: &std::path::Path| std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned();
        s.entries.insert(key(&p_a), crate::state::StateEntry {
            cursor: 7, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq: 1,
            folds: vec![], block: None, ..Default::default() });
        let cfg = crate::config::Config::default();
        drain_session_migrations(&mut s, &mut e, &cfg);

        assert!(s.entries.contains_key(&key(&p_c)),
            "the chain must land at the FINAL path — dispatch-time capture strands it at /b");
        assert_eq!(s.entries[&key(&p_c)].cursor, 7, "carrying the original cursor through both hops");
        assert!(!s.entries.contains_key(&key(&p_a)), "no stale source entry");
        assert!(!s.entries.contains_key(&key(&p_b)), "no stranded intermediate entry");
        assert_eq!(s.entries.len(), 1, "exactly ONE entry survives");
        for f in [&p_a, &p_b, &p_c] { let _ = std::fs::remove_file(f); }
    }

    #[test]
    fn a_migration_whose_source_is_gone_is_a_silent_no_op() {
        // Hygiene, not a durability guarantee: never an error, never a reason to fail a
        // persist.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let cfg = crate::config::Config::default();
        let mut s = crate::state::SessionState::default();
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/never-existed.md".into(), to: "/z.md".into() });
        drain_session_migrations(&mut s, &mut e, &cfg);
        assert!(s.entries.is_empty(), "nothing invented");
        assert!(e.pending_session_migrations.is_empty(), "still drained");
    }

    #[test]
    fn open_into_current_replaces_with_fresh_id_and_clean() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-oic-{}.md", std::process::id()));
        std::fs::write(&p, "opened\n").unwrap();
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        let old_id = e.active().id;
        open_into_current(&mut e, &crate::fsx::RealFs, &p);
        assert_ne!(e.active().id, old_id, "fresh id → stale in-flight jobs for old buffer are ignored");
        assert_eq!(e.active().document.buffer.to_string(), "opened\n");
        assert!(!e.active().document.dirty());
        let _ = std::fs::remove_file(&p);
    }

    /// A17 T4: an open-into-current failure (path is a directory → OpenError::IsDir) must
    /// land Sticky/Error — surviving a later Info ack (Q1), not clearing on the next keystroke.
    #[test]
    fn open_into_current_failure_is_a_sticky_error_that_survives_a_later_info() {
        use crate::editor::Editor;
        let dir = std::env::temp_dir().join(format!("wc-oic-isdir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        open_into_current(&mut e, &crate::fsx::RealFs, &dir);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_browser_enter_on_file_opens_it_when_clean() {
        use crate::editor::Editor;
        let dir = std::env::temp_dir().join(format!("wc-fbopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.md"), "loaded\n").unwrap();
        let mut e = Editor::new_from_text("clean\n", None, (80, 24)); // clean
        let (tx, _rx) = std::sync::mpsc::channel();
        e.open_file_browser(&crate::test_support::test_fs(), &tx, dir.clone());
        // select "note.md" and simulate Enter via the browser's open path:
        open_into_current(&mut e, &crate::fsx::RealFs, &dir.join("note.md")); // the clean-path the Enter handler takes
        assert_eq!(e.active().document.buffer.to_string(), "loaded\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_restores_when_identity_matches_and_clamps_when_not() {
        // unit-test the resume decision helper directly (no TTY):
        // apply_resume(entry, current_identity, doc_len) -> Option<(cursor,scroll)>
        use crate::state::StateEntry;
        let e = StateEntry { cursor: 4, scroll: 2, marks: Default::default(), mtime: 10, size: 20, seq: 0, folds: vec![], block: None, ..Default::default() };
        // identity match → restore (clamped to doc_len)
        assert_eq!(apply_resume(&e, (10,20), 100), Some((4,2)));
        assert_eq!(apply_resume(&e, (10,20), 3), Some((3,2)), "cursor clamped to doc_len");
        // identity mismatch → discard
        assert_eq!(apply_resume(&e, (11,20), 100), None);
    }

    #[test]
    fn load_marks_from_entry_populates_clamped() {
        use std::collections::BTreeMap;
        // No trailing newline so clamp_snap(999) == buffer.len() == 11.
        let mut e = Editor::new_from_text("hello world", None, (80, 24));
        let mut marks = BTreeMap::new();
        marks.insert("a".to_string(), 6usize);
        marks.insert("b".to_string(), 999usize); // past EOF → clamped to len
        let entry = crate::state::StateEntry { cursor: 0, scroll: 0, marks, mtime: 0, size: 0, seq: 1, folds: vec![], block: None, ..Default::default() };
        load_marks_from_entry(&mut e, &entry);
        assert_eq!(e.active().marks.get(&'a'), Some(&6));
        assert_eq!(e.active().marks.get(&'b'), Some(&e.active().document.buffer.len()));
    }

    /// Task 5 (9A): marked block persists and restores across sessions.
    /// Mirrors `load_marks_from_entry_populates_clamped` — tests the restore code path
    /// directly (analogous to how marks/folds restore tests work).
    #[test]
    fn marked_block_persists_and_restores_under_matching_identity() {
        use crate::editor::{Editor, MarkedBlock};
        use crate::state::StateEntry;

        // Construct an entry with a block — compile fails until StateEntry has `block`.
        let entry = StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(),
            mtime: 10, size: 20, seq: 1, folds: vec![],
            block: Some((3, 8)),
            ..Default::default()
        };

        // ── matching identity: guard passes → block restores with hidden=false ──
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        let doc_len = e.active().document.buffer.len();
        assert!(
            apply_resume(&entry, (10, 20), doc_len).is_some(),
            "identity match → guard passes"
        );
        // Simulate what restore_resume does after the staleness guard:
        if let Some((s, en)) = entry.block {
            e.active_mut().marked_block = Some(MarkedBlock { start: s, end: en, hidden: false });
        }
        assert_eq!(
            e.active().marked_block,
            Some(MarkedBlock { start: 3, end: 8, hidden: false }),
            "block restores with hidden=false under matching identity"
        );

        // ── mismatching identity: guard rejects → block NOT restored ──
        //
        // Previously vacuous: e2 was a fresh Editor and `marked_block` was never
        // set, so the final assert was trivially true regardless of the guard.
        //
        // Hardened: we now drive the same conditional-restore path that
        // restore_resume uses (apply_resume as gate → block only if Some).
        // The block-application code IS present; the staleness guard (mtime 99 ≠
        // stored 10) stops it.  If apply_resume were made to ignore mismatches
        // (always return Some), the final assert would flip to RED.
        let mut e2 = Editor::new_from_text("hello world\n", None, (80, 24));
        let doc_len2 = e2.active().document.buffer.len();
        let guard = apply_resume(&entry, (99, 20), doc_len2);
        assert!(guard.is_none(), "identity mismatch → guard rejects");
        // Mirror restore_resume: set block only when guard passes.
        if guard.is_some() {
            if let Some((s, en)) = entry.block {
                e2.active_mut().marked_block =
                    Some(MarkedBlock { start: s, end: en, hidden: false });
            }
        }
        // Non-vacuous: restore code is present above; guard prevented it from running.
        assert!(e2.active().marked_block.is_none(), "block discarded on mismatch — guard blocked restore");
    }

    /// Task 5 (9A) regression: an out-of-range persisted block (dirty-quit → reopen
    /// shorter file, guard still passes) must NOT reach `buffer.slice()` and panic.
    /// Drives the REAL restore helper (`load_block_from_entry`, the exact code
    /// `restore_resume` uses). Pre-fix this restored `end=8 > len=4` and the
    /// `block_copy`/`block_delete` below asserted in `slice()` → panic.
    #[test]
    fn restore_clamps_out_of_range_block_no_slice_panic() {
        use crate::editor::Editor;
        use crate::state::StateEntry;

        // Short buffer (len 4) but the persisted block end (8) is past EOF.
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let len = e.active().document.buffer.len();
        assert_eq!(len, 4);
        let entry = StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(),
            mtime: 10, size: 20, seq: 1, folds: vec![],
            block: Some((4, 8)), // start at EOF, end beyond EOF
            ..Default::default()
        };

        // Real production restore path (post-staleness-guard).
        load_block_from_entry(&mut e, &entry);

        // Clamped to <= len, or dropped entirely — never out of range.
        if let Some(b) = e.active().marked_block {
            assert!(b.end <= len, "restored block end clamped to buffer len");
            assert!(b.start <= b.end, "restored block normalized");
        }
        // (4,8) clamps to (4,4) which collapses → dropped.
        assert!(e.active().marked_block.is_none(), "collapsed block dropped");

        // The KEY assertion: block ops do not panic in slice() with the restored state.
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        crate::blocks_marked::block_delete(&mut e, &TestClock(0));

        // And a genuinely out-of-range END that does NOT collapse is still clamped.
        let mut e2 = Editor::new_from_text("abc\n", None, (80, 24));
        let entry2 = StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(),
            mtime: 10, size: 20, seq: 1, folds: vec![],
            block: Some((1, 99)), // start in-range, end far past EOF
            ..Default::default()
        };
        load_block_from_entry(&mut e2, &entry2);
        let b = e2.active().marked_block.expect("non-collapsing block restored");
        assert!(b.end <= e2.active().document.buffer.len(), "end clamped to len");
        // Must not panic:
        crate::blocks_marked::block_copy(&mut e2, &TestClock(0));
        crate::blocks_marked::block_delete(&mut e2, &TestClock(0));
    }

    #[test]
    fn restore_scratch_loads_text_and_clamps_cursor() {
        let mut e = crate::editor::Editor::new_from_text("doc\n", None, (40, 10));
        e.install_scratch();
        let st = crate::state::ScratchState { text: "hello".into(), cursor: 999 }; // out of range
        restore_scratch(&mut e, &st);
        let sid = e.scratch_id.unwrap();
        let sb = e.by_id(sid).unwrap();
        assert_eq!(sb.document.buffer.to_string(), "hello");
        assert_eq!(sb.document.selection.primary().head, 5, "cursor clamped to len");
    }

    // -------------------------------------------------------------------------
    // C5 Task 25: DocumentId mint-and-stamp
    // -------------------------------------------------------------------------

    #[test]
    fn persist_session_stamps_the_active_documents_id() {
        // FAIL-VERIFY: remove the `id:` assignment in `persist_session`, watch this fail.
        let p = std::env::temp_dir().join(format!("wc-idstamp-{}.md", std::process::id()));
        std::fs::write(&p, b"x\n").expect("seed");
        let e = crate::editor::Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
        let expected = e.active().document.id.to_hex();
        let mut s = crate::state::SessionState::default();
        let cfg = crate::config::Config::default();
        persist_session_for_test(&mut s, &e, &cfg, 1);
        let key = std::fs::canonicalize(&p).expect("canon").to_string_lossy().into_owned();
        assert_eq!(s.entries[&key].id.as_deref(), Some(expected.as_str()),
            "the entry must carry the ACTIVE document's id, not a fresh or absent one");
        let _ = std::fs::remove_file(&p);
    }

    // -------------------------------------------------------------------------
    // Effort 6, Task 2: scratch persistence
    // -------------------------------------------------------------------------

    #[test]
    fn persist_session_captures_scratch_even_when_active_unnamed() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10)); // active unnamed
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "stash".into())], 0);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(5));
        e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        let mut session = crate::state::SessionState::default();
        let cfg = crate::config::Config::default();
        crate::session_restore::persist_session_for_test(&mut session, &e, &cfg, 1);
        assert_eq!(session.scratch.as_ref().unwrap().text, "stash");
    }

    #[test]
    fn persist_session_clears_stale_scratch_when_oversized() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10));
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        // Make the live scratch buffer oversized (> MAX_SESSION_BYTES).
        let big = "x".repeat(crate::limits::MAX_SESSION_BYTES + 1);
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, big)], 0);
        let txn = wordcartel_core::history::Transaction::new(cs);
        e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        // Session carries a STALE scratch loaded from a previous launch.
        let mut session = crate::state::SessionState {
            scratch: Some(crate::state::ScratchState { text: "old stale".into(), cursor: 0 }),
            ..Default::default()
        };
        let cfg = crate::config::Config::default();
        crate::session_restore::persist_session_for_test(&mut session, &e, &cfg, 1);
        assert!(session.scratch.is_none(),
            "oversized live scratch must CLEAR the stale loaded scratch, not resurrect it");
    }
}
