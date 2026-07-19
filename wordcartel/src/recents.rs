//! Recents: the rescue path for "I can't find my file".
//!
//! Nearly free — `SessionState.entries` is ALREADY an LRU-ranked (`seq`),
//! canonical-path-keyed map that the editor maintains on every save. This module only reads
//! it and turns it into rows the existing file-picker overlay (`file_browser.rs`) can show,
//! filter, and open — no new listing/painting/mouse machinery.

/// One row in the recents list. Missing files stay VISIBLE but are not selectable — a
/// writer whose file moved needs to see that it is gone, not to find a shorter list.
///
/// `pub`, not `pub(crate)`: it rides inside `Msg::RecentsProbed`, and `Msg` is `pub` — the
/// same reason `filter::Disposition`/`export::ExportResult`/`transform::TransformKind` are
/// `pub` rather than crate-private (a narrower type embedded in a `pub` enum variant is a
/// `private_interfaces` build-clean GATE failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRow {
    pub path: std::path::PathBuf,
    pub available: bool,
}

/// Rank `session.entries` by `seq` descending — it is already an LRU-ordered,
/// canonical-path-keyed map, so recents is nearly free. `fs` is a parameter (not `RealFs`
/// reached for directly) so tests, and the availability-probe thread below, can drive this
/// through the same seam the rest of the crate uses.
pub(crate) fn rows_from(session: &crate::state::SessionState, fs: &dyn crate::fsx::Fs)
    -> Vec<RecentRow>
{
    let mut v: Vec<(u64, RecentRow)> = session.entries.iter().map(|(k, e)| {
        let path = std::path::PathBuf::from(k);
        let available = crate::fsx::is_file_via(fs, &path);
        (e.seq, RecentRow { path, available })
    }).collect();
    v.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq)); // most-recent first
    v.into_iter().map(|(_, r)| r).collect()
}

/// `session.entries`' keys, ranked by `seq` descending — the pure half of `rows_from`,
/// with no filesystem access. `open_recent` uses this to show every row immediately,
/// before availability has been probed at all.
pub(crate) fn ranked_paths(session: &crate::state::SessionState) -> Vec<std::path::PathBuf> {
    let mut v: Vec<(u64, std::path::PathBuf)> = session.entries.iter()
        .map(|(k, e)| (e.seq, std::path::PathBuf::from(k)))
        .collect();
    v.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    v.into_iter().map(|(_, p)| p).collect()
}

/// Open the recents picker. Rows route through the picker's ordinary `EnterOutcome::Open`
/// arm into `workspace::open_as_new_buffer`, inheriting the dirty-guard and resume
/// behaviour; unavailable rows are rendered marked and refuse selection rather than
/// vanishing — no special-casing needed anywhere else.
///
/// Availability is computed OFF the UI thread. The spec puts the existence check on the
/// listing thread — a recents list spanning a hung network mount would otherwise block the
/// input loop on `is_file_via` for every row, which is the exact hazard §6.3 exists to
/// prevent. `open_recents_pending` shows every row immediately (optimistically selectable);
/// the rows are re-marked in place when the spawned probe's `Msg::RecentsProbed` lands,
/// under the SAME epoch discipline `start_listing`/`apply_listing_done` use for directory
/// listings.
pub(crate) fn open_recent(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>)
{
    // `state_dir()` is directory provisioning and stays raw; the session READ below goes
    // through the seam.
    let session = match crate::swap::state_dir() {
        Ok(dir) => crate::state::load_in_with_fs(&**fs, &dir),
        Err(_) => crate::state::SessionState::default(),
    };
    open_recent_from(editor, fs, msg_tx, session);
}

/// Directory-injectable form of `open_recent`, mirroring `state::load_in`/`save_in`.
///
/// `open_recent` was the last session-store reader with no directory seam, so a test
/// exercising it had to read — and therefore restore — the DEVELOPER'S real
/// `$XDG_STATE_HOME/wordcartel/session.toml`. Load-then-restore is not exclusive: the
/// `persist_session_for_test` tests write that same file on a sibling thread of the same
/// process, so an interleave could silently discard their write. Tests point this at a temp
/// dir instead and touch nothing ambient.
#[cfg(test)]
pub(crate) fn open_recent_in(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, dir: &std::path::Path)
{
    let session = crate::state::load_in_with_fs(&**fs, dir);
    open_recent_from(editor, fs, msg_tx, session);
}

/// Shared core of `open_recent` / `open_recent_in`: everything downstream of WHERE the
/// session came from. Split so the two entries can never diverge in ranking, epoch
/// discipline, or the off-thread probe.
fn open_recent_from(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, session: crate::state::SessionState)
{
    // Paths are ranked here so the writer sees the list at once; the per-row `is_file`
    // probe (the only part that touches the filesystem beyond the already-loaded session)
    // goes to the thread.
    let paths = ranked_paths(&session);
    if paths.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "No recent files");
        return;
    }
    editor.open_recents_pending(paths);      // shows rows immediately, availability TBD
    let epoch = crate::file_browser::next_epoch();
    if let Some(fb) = editor.file_browser.as_mut() { fb.awaiting_epoch = epoch; }
    let fs = std::sync::Arc::clone(fs);
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        // Reuse `rows_from` rather than re-deriving the same ranking a second time by
        // hand — it produces the identical order `ranked_paths` already showed (both walk
        // the same `BTreeMap` and sort on the same key), plus the availability flag.
        let rows = rows_from(&session, &*fs);
        let _ = tx.send(crate::app::Msg::RecentsProbed { epoch, rows });
    });
}

/// Apply a completed availability probe. Discards a stale epoch — a probe for a recents
/// list the writer has already closed and reopened must never re-mark the CURRENT rows.
///
/// Re-marks IN PLACE rather than calling `open_recents` again. Reopening would run
/// `close_all`, reset `query` to empty, and reset `selected` — so a probe landing a beat
/// after the writer started typing would wipe their filter and move their cursor. The probe
/// carries availability and nothing else; it must change availability and nothing else.
///
/// Re-marks BOTH `fb.listing` and `fb.entries`. `entries` alone is not enough: `rederive`
/// now re-derives `entries` fresh from `listing` on every keystroke (the Task 23 ratchet
/// fix), so a `listing` left stale would let an unavailable row's `Unknown` marking silently
/// reappear as `File` the moment the writer's next keystroke re-filters — the probe's result
/// would look applied and then un-apply itself.
pub(crate) fn apply_recents_probed(editor: &mut crate::editor::Editor, epoch: u64,
    rows: Vec<RecentRow>)
{
    let Some(fb) = editor.file_browser.as_mut() else { return };
    if fb.awaiting_epoch != epoch { return; }
    let unavailable: std::collections::HashSet<String> = rows.iter()
        .filter(|r| !r.available)
        .map(|r| r.path.to_string_lossy().into_owned())
        .collect();
    for entry in fb.listing.iter_mut() {
        if unavailable.contains(&entry.name) { entry.kind = crate::fsx::EntryKind::Unknown; }
    }
    for entry in fb.entries.iter_mut() {
        if unavailable.contains(&entry.name) { entry.kind = crate::fsx::EntryKind::Unknown; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::press_char_fb;

    #[test]
    fn rows_are_seq_ranked_and_missing_files_stay_visible_but_unavailable() {
        let d = std::env::temp_dir().join(format!("wc-recents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let live = d.join("live.md");
        std::fs::write(&live, b"x").expect("seed");
        let gone = d.join("gone.md");

        let mut s = crate::state::SessionState::default();
        let entry = |seq: u64| crate::state::StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq,
            folds: vec![], block: None, ..Default::default() };
        s.entries.insert(gone.to_string_lossy().into_owned(), entry(9));
        s.entries.insert(live.to_string_lossy().into_owned(), entry(3));

        let rows = rows_from(&s, &crate::fsx::RealFs);
        assert_eq!(rows.len(), 2, "a missing file is SHOWN, not dropped — a shorter list \
            would hide the fact that it moved");
        assert_eq!(rows[0].path, gone, "ranked by seq descending (9 before 3)");
        assert!(!rows[0].available, "and marked unavailable");
        assert_eq!(rows[1].path, live);
        assert!(rows[1].available);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recents_open_unblocked_and_a_stale_probe_never_re_marks_the_rows() {
        // Two properties in one arrangement because they share it:
        //  (a) OPENING shows every row selectable immediately — the writer never waits on one
        //      `stat` per row, which is the whole reason the probe is off-thread (§6.3).
        //  (b) A STALE probe is DISCARDED. Without the epoch check, a slow probe for a list
        //      the writer already closed and reopened would re-mark the CURRENT rows with the
        //      PREVIOUS list's availability — rows greying out for the wrong files.
        //
        // FAIL-VERIFY (two): (a) make `open_recents_pending` probe inline with `is_file_via`
        // and mark rows `Unknown` — the pre-probe assertion fails; (b) delete the
        // `epoch != fb.awaiting_epoch` guard from the arm — the stale assertion fails.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let gone = std::path::PathBuf::from("/nonexistent/ch1.md");
        e.open_recents_pending(vec![gone.clone()]);

        // (a) Pre-probe: shown, and selectable — NOT yet marked unavailable.
        let fb = e.file_browser.as_ref().expect("the picker opened");
        assert_eq!(fb.entries.len(), 1, "the row is on screen before any probe lands");
        assert_eq!(fb.entries[0].kind, crate::fsx::EntryKind::File,
            "availability is UNKNOWN-but-optimistic pre-probe, so the open never blocked");
        let live_epoch = fb.awaiting_epoch;

        // (b) A probe stamped with a DIFFERENT epoch must change nothing.
        crate::recents::apply_recents_probed(&mut e, live_epoch.wrapping_sub(1),
            vec![crate::recents::RecentRow { path: gone.clone(), available: false }]);
        assert_eq!(e.file_browser.as_ref().unwrap().entries[0].kind,
            crate::fsx::EntryKind::File, "a STALE probe is discarded, not applied");

        // (c) A live probe re-marks availability and NOTHING ELSE. The writer types while the
        // probe is in flight; when it lands their filter and cursor must survive. This is why
        // the arm re-marks in place instead of calling `open_recents` again — that path runs
        // `close_all` and resets `query`, silently eating the keystrokes they just made.
        //
        // FAIL-VERIFY: implement the arm as `editor.open_recents(rows)`, watch the query
        // assertion fail while the availability one still passes.
        e.file_browser.as_mut().unwrap().query.push_str("ch1");
        crate::recents::apply_recents_probed(&mut e, live_epoch,
            vec![crate::recents::RecentRow { path: gone, available: false }]);
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.entries[0].kind, crate::fsx::EntryKind::Unknown,
            "the LIVE probe marks the row unavailable");
        assert_eq!(fb.query, "ch1", "and leaves the in-flight filter the writer typed intact");
    }

    /// C5 review finding M7. `apply_recents_probed` marks a recent whose file has vanished as
    /// `EntryKind::Unknown` so that Enter refuses it — but it then inherited the shared
    /// `Unknown` wording, "type could not be determined". That is a LISTING fact, reported
    /// about a row that was never listed: nothing probed this file's type and failed, the file
    /// simply is not there any more. A writer told their manuscript has an indeterminate type
    /// goes looking for a filesystem problem instead of a moved file.
    ///
    /// Driven through the real Enter path, and paired against a genuine directory listing's
    /// `Unknown` so the fix cannot be "reword the arm for everyone" — the two facts stay
    /// distinct, which is the property the `Unknown` arm exists to preserve.
    ///
    /// FAIL-VERIFY: drop the `recents` arm from `classify_enter`, watch the status assertion
    /// fail with "type could not be determined". Confirmed, then reverted.
    #[test]
    fn an_unavailable_recent_is_refused_as_gone_not_as_indeterminate() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let gone = std::path::PathBuf::from("/nonexistent/ch1.md");
        e.open_recents_pending(vec![gone.clone()]);
        let live_epoch = e.file_browser.as_ref().expect("picker open").awaiting_epoch;
        crate::recents::apply_recents_probed(&mut e, live_epoch,
            vec![crate::recents::RecentRow { path: gone, available: false }]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        crate::test_support::press_enter_fb(&mut e, &fs, &tx);

        let status = e.status_text().to_lowercase();
        assert!(status.contains("no longer available"),
            "the refusal must say the file is gone: {status:?}");
        assert!(!status.contains("could not be determined"),
            "not that its type is indeterminate — nothing failed to classify it: {status:?}");
        assert!(e.file_browser.is_some(), "and the picker stays open to pick something else");

        // The listing fact keeps its own wording: a real `Unknown` in a real directory is a
        // classification failure and must NOT inherit the recents phrasing.
        let listed = crate::file_browser::classify_enter(
            &crate::file_browser::FileEntry {
                name: "mystery".into(), kind: crate::fsx::EntryKind::Unknown,
                is_symlink: false, broken: false },
            std::path::Path::new("/tmp"), false);
        match listed {
            crate::file_browser::EnterOutcome::Refuse(m) =>
                assert!(m.contains("type could not be determined"),
                    "a listed unclassifiable entry keeps the classification wording: {m:?}"),
            other => panic!("an Unknown entry must still be refused, got {other:?}"),
        }
    }

    #[test]
    fn typing_narrows_the_recents_list_rather_than_clearing_it() {
        // THE reason `Recents` is an explicit variant. The rejected design used an early
        // return in `rederive`, which preserved the rows but could not FILTER them — typing
        // would have silently done nothing. This is the assertion that would have caught it.
        //
        // FAIL-VERIFY (two, because there are two ways to break this): (a) make `rederive`'s
        // Recents arm return early instead of ranking — all four rows survive the filter;
        // (b) drop the printable-char arm from `file_browser_intercept`'s Recents branch —
        // the query never fills and all four survive. The keystroke goes through the REAL
        // intercept via `press_char_fb`: pushing to `fb.query` and calling `rederive` by hand
        // proves the ranker works, NOT that typing reaches it, and (b) would pass vacuously.
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.open_recents(vec![
            RecentRow { path: "/w/chapter-one.md".into(), available: true },
            RecentRow { path: "/w/chapter-two.md".into(), available: true },
            RecentRow { path: "/w/notes.md".into(),       available: true },
            RecentRow { path: "/w/outline.md".into(),     available: false },
        ]);
        assert_eq!(e.file_browser.as_ref().expect("open").entries.len(), 4, "precondition");

        for c in "chapter".chars() { press_char_fb(&mut e, &fs, &tx, c); }

        let names: Vec<String> = e.file_browser.as_ref().unwrap()
            .entries.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.len(), 2, "the list NARROWED, it did not clear or stay whole: {names:?}");
        assert!(names.iter().all(|n| n.contains("chapter")), "{names:?}");
    }

    #[test]
    fn backspacing_the_query_widens_the_recents_list_back_out() {
        // THE RATCHET. Task 23's `rederive` Recents arm filtered `fb.entries` IN PLACE and
        // wrote the narrowed result back into `fb.entries` — so the already-narrowed list
        // became its own source on the next keystroke. Typing narrowed correctly (the
        // sibling test above); backing OUT never widened back, because there was nowhere
        // left to widen FROM. `fb.listing` is the fix: an immutable cache, populated once at
        // open by `open_recents`, that every keystroke re-derives `fb.entries` from fresh —
        // exactly the invariant `Select`/`Destination` already hold.
        //
        // FAIL-VERIFY: revert `rederive`'s Recents arm to filter `&fb.entries` instead of
        // `&fb.listing` (the Task 23 shape) — this test fails; the list stays at 2 rows
        // after Backspace empties the query, instead of widening back to 4.
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.open_recents(vec![
            RecentRow { path: "/w/chapter-one.md".into(), available: true },
            RecentRow { path: "/w/chapter-two.md".into(), available: true },
            RecentRow { path: "/w/notes.md".into(),       available: true },
            RecentRow { path: "/w/outline.md".into(),     available: false },
        ]);
        assert_eq!(e.file_browser.as_ref().expect("open").entries.len(), 4, "precondition");

        // Narrow to 2, through the real intercept.
        for c in "chap".chars() { press_char_fb(&mut e, &fs, &tx, c); }
        assert_eq!(e.file_browser.as_ref().unwrap().entries.len(), 2,
            "precondition: narrowed before backspacing");

        // Backspace all four typed characters, through the real intercept — `query` empties.
        for _ in 0..4 {
            crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Backspace);
        }
        let fb = e.file_browser.as_ref().expect("still open");
        assert_eq!(fb.query, "", "the query itself correctly empties");

        let names: Vec<String> = fb.entries.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.len(), 4,
            "the list must WIDEN back to its full length once the filter is gone: {names:?}");
        // Not merely the right COUNT — the right ROWS, in the original order, including the
        // unavailable one. A count-only assertion would pass even if `rederive` widened back
        // to 4 of the WRONG rows.
        assert_eq!(names, vec![
            "/w/chapter-one.md", "/w/chapter-two.md", "/w/notes.md", "/w/outline.md",
        ], "the full, ORIGINAL list, not merely four rows: {names:?}");
    }
}
