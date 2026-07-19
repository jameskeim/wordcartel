//! Destination-mode commit semantics: what Enter MEANS when the writer is naming a file.
//!
//! Split from `file_browser.rs` on one axis of change. This is the highest-risk logic in
//! C5 — the only place where an error produces silent overwrite or save-to-nowhere — so it
//! lives alone, is pure, and is tested row by row.

use crate::file_browser::FileEntry;
use crate::fsx::{EntryKind, Fs};
use std::path::{Path, PathBuf};

/// What Enter/the footer preview should do with the current field + highlighted entry.
/// `file_browser::footer_target` calls `classify_destination_enter` directly so the footer can
/// never state a destination Enter will not actually reach — Task 21 additionally wires this
/// into the LIVE Enter/commit path (`Commit`/`Descend` are not yet actioned there).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Descend(PathBuf),
    Commit {
        /// The absolute path Enter targets, before extension policy.
        path: PathBuf,
        /// TRUE only for Row 2 — the empty-field commit onto a HIGHLIGHTED EXISTING FILE.
        ///
        /// Extension policy (§8) is "a pure classification function over the **field text**",
        /// and a Row-2 commit has no field text: the highlighted file's own name IS the
        /// target, chosen off the screen by the writer. Piping it through policy anyway
        /// retargeted an existing extensionless `README` to `README.md` — a DIFFERENT file,
        /// created silently, with no overwrite confirm (the new path did not exist) and no
        /// footer disclosure (the field was empty), which is exactly the harm Row 2's own
        /// contract says the overwrite-confirm is there to prevent.
        from_highlight: bool,
    },
    Nothing,
}

/// Resolve a field value against the directory the writer is LOOKING AT.
///
/// Deliberately NOT `prompts::expand_path`: that joins relative input onto
/// `std::env::current_dir()`, which is invisible to someone reading a directory listing.
/// Joining cwd would put the file somewhere the picker never showed them.
///
/// 1. `~/`-prefixed -> home-relative.
/// 2. absolute      -> as typed.
/// 3. otherwise     -> joined onto `dir`, NOT onto cwd.
pub(crate) fn resolve_field(dir: &Path, field: &str) -> PathBuf {
    let t = field.trim();
    if let Some(rest) = t.strip_prefix("~/") {
        return dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(t));
    }
    let p = PathBuf::from(t);
    if p.is_absolute() { p } else { dir.join(p) }
}

/// The four-row Enter decision table (spec §7.2). Evaluated top to bottom; first match wins.
///
/// | # | Condition                                                      | Action              |
/// |---|------------------------------------------------------------------|---------------------|
/// | 1 | highlighted entry is a dir (incl "..") AND (navigated OR field empty) | Descend      |
/// | 2 | field empty AND highlighted entry is a file                    | Commit to that file |
/// | 3 | field resolves to an EXISTING directory                         | Descend into it     |
/// | 4 | otherwise                                                       | Commit dir + field  |
///
/// The sole source of truth for "what will Enter do" — `file_browser::footer_target` calls this
/// directly so the resolved-target footer can never state a destination Enter will not actually
/// reach (a bare field naming an EXISTING directory must be shown as a descend, never a write).
///
/// `highlight_navigated` (the caller passes `FileBrowser::highlight_is_navigated()` — see its
/// doc comment and `FileBrowser::navigated_name`) is what Row 1 gates on: true only when the
/// entry CURRENTLY highlighted is the one the writer DELIBERATELY chose — a nav key that
/// actually moved `selected` (arrow keys, Home/End, PageUp/PageDown), a click, or a wheel
/// scroll, re-validated by name against whatever is highlighted NOW so a re-filter that slides
/// a different entry under the same index cannot inherit a choice it never received. Without
/// this gate, `filter_and_rank` unconditionally pins the synthetic ".." row (or, at filesystem
/// root, whichever real entry sorts first — directories sort before files) at `entries[0]`, and
/// `FileBrowser::selected` initializes to 0 — so the ordinary "type a name, press Enter"
/// sequence would hit Row 1 on a highlight the writer never touched and silently descend
/// instead of committing.
pub(crate) fn classify_destination_enter(
    fs: &dyn Fs,
    dir: &Path,
    field: &str,
    highlighted: Option<&FileEntry>,
    highlight_navigated: bool,
) -> CommitOutcome {
    let trimmed = field.trim();

    // Row 1 — a highlighted directory descends, EVEN with a non-empty field, so the writer
    // keeps their filename while navigating. Gated on `highlight_navigated || trimmed.is_empty()`:
    // the "keep the filename while navigating" feature stays intact (the writer moved the
    // highlight there themselves), and a bare Enter with nothing typed still navigates on
    // whatever is highlighted (an ordinary browse gesture) — but a NON-empty field with an
    // UNTOUCHED default highlight falls through to Row 3/4 instead of silently descending.
    if let Some(e) = highlighted {
        if matches!(e.kind, EntryKind::Dir) && (highlight_navigated || trimmed.is_empty()) {
            let target = if e.name == ".." {
                dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf())
            } else {
                dir.join(&e.name)
            };
            return CommitOutcome::Descend(target);
        }
    }

    // Row 2 — an empty field commits onto the highlighted FILE. Explicit overwrite intent:
    // it takes navigating there AND pressing Enter with a visibly empty field, and it still
    // raises the overwrite-confirm downstream.
    if trimmed.is_empty() {
        return match highlighted {
            Some(e) if matches!(e.kind, EntryKind::File) => {
                CommitOutcome::Commit { path: dir.join(&e.name), from_highlight: true }
            }
            // Other/Unknown are refused in select mode and are not commit targets here
            // either — we do not know they are writable regular files.
            _ => CommitOutcome::Nothing,
        };
    }

    let resolved = resolve_field(dir, trimmed);

    // Row 3 — the one genuinely ambiguous case, resolved TOWARD DESCEND. A directory named
    // `chapter-one` in the list while Enter creates a FILE `chapter-one.md` beside it is the
    // worse surprise; descend is recoverable in one keystroke ('..'), a misplaced file is not.
    if matches!(fs.stat(&resolved), Ok(st) if st.is_dir) {
        return CommitOutcome::Descend(resolved);
    }

    // Row 4 — the ordinary case.
    CommitOutcome::Commit { path: resolved, from_highlight: false }
}

/// Extensions that mean "this is an export, not a save".
const OUTPUT_EXTS: &[&str] = &["docx", "pdf", "html", "tex"];

/// The verdict `apply_extension_policy` reaches for a SAVE destination's extension.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtVerdict {
    /// Append `.md` — the name had no extension.
    Defaulted(PathBuf),
    /// A recognized OUTPUT extension. Refuse the save and offer Export, carrying the typed
    /// path forward so the writer's intent is not thrown away.
    Redirect { path: PathBuf, ext: String },
    /// Any other extension — honoured silently.
    Honoured(PathBuf),
    /// The typed name has a TRAILING SEPARATOR (e.g. `sub/`) — it names a directory, not a
    /// file. Refuse outright: unlike `Redirect`, there is no alternate flow to offer, only
    /// the field for the writer to fix.
    Refused(PathBuf),
}

/// F4's default-and-redirect policy for SAVE destinations.
///
/// Redirect is only defensible because export now HAS a destination (spec §9) — before C5,
/// "use Export instead" was advice with nowhere to go.
///
/// Never applied in select mode, and never to an export destination (whose extension is
/// fixed by the format).
pub(crate) fn apply_extension_policy(path: &Path) -> ExtVerdict {
    // A TRAILING SEPARATOR (`sub/`) names a directory the writer is asking to create or
    // enter, not a file — but `Path::file_name()` (and therefore `extension()`) silently
    // strips it, so `/d/sub/` and `/d/sub` are indistinguishable once parsed. This check
    // must run on the raw string BEFORE any `Path` method sees it, or the writer ends up
    // with a hidden `.md` file created INSIDE the directory they named (the Row-4
    // fallthrough in `classify_destination_enter` routes a not-yet-existing directory-like
    // name here rather than to Descend).
    if path.to_string_lossy().chars().last().is_some_and(std::path::is_separator) {
        return ExtVerdict::Refused(path.to_path_buf());
    }
    // `Path::extension()` returns `Some("")` for a TRAILING-DOT name like `notes.` — there
    // IS an embedded dot, so it is not None, and the part after it is empty. Treating that
    // as "has an extension" would take the Honoured arm and skip defaulting, leaving the
    // writer with an extensionless `notes.` file. Filter the empty case into the None arm.
    match path.extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty()) {
        Some(ext) => {
            let lower = ext.to_ascii_lowercase();
            if OUTPUT_EXTS.contains(&lower.as_str()) {
                ExtVerdict::Redirect { path: path.to_path_buf(), ext: lower }
            } else {
                ExtVerdict::Honoured(path.to_path_buf())
            }
        }
        None => {
            let s = path.to_string_lossy();
            // A dotfile has no extension AND must not be defaulted — its file_name starts
            // with '.' and contains no further dot.
            let is_dotfile = path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if is_dotfile {
                return ExtVerdict::Honoured(path.to_path_buf());
            }
            let trimmed = s.trim_end_matches('.');
            ExtVerdict::Defaulted(PathBuf::from(format!("{trimmed}.md")))
        }
    }
}

/// The `Tab` gesture: replace the field with a highlighted file's name. Returns nothing and
/// touches no path — it CANNOT commit, which is the point. Overwrite becomes: highlight,
/// Tab (name lands, footer shows the resolved target), Enter (overwrite-confirm).
pub(crate) fn copy_name_into_field(field: &mut String, field_cursor: &mut usize, name: &str) {
    field.clear();
    field.push_str(name);
    *field_cursor = field.len();
}

/// Execute a destination-mode Enter. THE single place a picker commit becomes a write.
///
/// Dispatches on `DestinationPurpose`, so adding a future destination consumer is one arm
/// the compiler demands rather than a new branch somewhere else.
#[allow(clippy::too_many_lines)] // a flat, exhaustive commit-outcome × purpose dispatch — the
// highest-risk logic in C5 (the module doc comment), kept in ONE function on purpose so the
// whole write decision is auditable in one place rather than split across call sites.
pub(crate) fn commit_destination(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let Some(fb) = editor.file_browser.as_ref() else { return };
    let crate::file_browser::BrowseMode::Destination { purpose, field, .. } = &fb.mode
        else { return };
    let purpose = purpose.clone();
    let dir = fb.dir.clone();
    let highlighted = fb.entries.get(fb.selected).cloned();
    let highlight_navigated = fb.highlight_is_navigated();

    match classify_destination_enter(&**fs, &dir, field, highlighted.as_ref(), highlight_navigated) {
        // Rows 1 and 3 — navigate, do not write. The listing lands asynchronously and
        // `apply_listing_done` commits `fb.dir` only on success.
        CommitOutcome::Descend(target) => {
            if let Some(fb) = editor.file_browser.as_mut() {
                crate::file_browser::start_listing(fb, target, fs, msg_tx);
            }
        }
        // Nothing to commit — an empty field with no usable highlight. A Sticky Warning,
        // matching what the retired `save_as_submit` empty-path arm produced.
        CommitOutcome::Nothing => {
            let noun = match purpose {
                crate::file_browser::DestinationPurpose::SaveAs => "save-as",
                crate::file_browser::DestinationPurpose::WriteBlock => "write block",
                crate::file_browser::DestinationPurpose::Export { .. } => "export",
            };
            editor.set_status_full(crate::status::StatusKind::Warning,
                format!("{noun}: empty path"),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            // Backing out of a drain's Save-As aborts the quit (Effort-6 Codex C2).
            if matches!(purpose, crate::file_browser::DestinationPurpose::SaveAs) {
                editor.pending_save_as = None;
                editor.quit_drain = None;
                editor.quit_drain_advance = false;
            }
        }
        CommitOutcome::Commit { path: raw, from_highlight } => {
            // Extension policy applies to SAVE destinations only — an export's extension is
            // fixed by the chosen format — and only to FIELD TEXT (§8). Row 2 has neither:
            // its target is a file the writer highlighted on screen, so its name is already
            // the answer and policy can only move the write somewhere else (see
            // `CommitOutcome::Commit::from_highlight`).
            let chosen = match &purpose {
                crate::file_browser::DestinationPurpose::Export { .. } => raw,
                _ if from_highlight => raw,
                _ => match apply_extension_policy(&raw) {
                    ExtVerdict::Defaulted(p) | ExtVerdict::Honoured(p) => p,
                    ExtVerdict::Redirect { path, ext } => {
                        // F4: refuse the save, explain, and carry the typed path into the
                        // export destination picker — advice with somewhere to go.
                        editor.set_status_full(crate::status::StatusKind::Warning,
                            format!("{ext} is an export format \u{2014} opening Export instead"),
                            crate::status::StatusLifetime::Sticky,
                            crate::status::StatusSource::Host, None);
                        // A Redirect IS an abandoned save — the write did not happen, and
                        // the writer is being offered a different feature. Same rule the
                        // `CommitOutcome::Nothing` empty-path arm above applies: abandoning
                        // a save-then must abort the drain, or a LATER save could `.take()`
                        // this stale action and fire a `Quit` the writer no longer wants
                        // (Critical-1, Task 21 fix round).
                        if matches!(purpose, crate::file_browser::DestinationPurpose::SaveAs) {
                            editor.pending_save_as = None;
                            editor.quit_drain = None;
                            editor.quit_drain_advance = false;
                        }
                        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(dir);
                        let field = path.file_name()
                            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        editor.open_destination_picker(fs, msg_tx,
                            crate::file_browser::DestinationPurpose::Export { ext },
                            dir, field);
                        return;
                    }
                    ExtVerdict::Refused(path) => {
                        editor.set_status_full(crate::status::StatusKind::Warning,
                            format!("{} \u{2014} names a directory, not a file", path.display()),
                            crate::status::StatusLifetime::Sticky,
                            crate::status::StatusSource::Host, None);
                        return;
                    }
                },
            };
            // Resolve through symlinks BEFORE any write is dispatched (§7.6.1).
            let resolved = match crate::fsx::resolve_write_destination(&**fs, &chosen) {
                Ok(r) => r,
                Err(crate::fsx::DestError::BrokenSymlink) => {
                    editor.set_status_full(crate::status::StatusKind::Warning,
                        format!("{}: destination symlink cannot be resolved", chosen.display()),
                        crate::status::StatusLifetime::Sticky,
                        crate::status::StatusSource::Host, None);
                    return;
                }
            };
            editor.file_browser = None;   // the picker's work is done

            // The overwrite-confirm names the RESOLVED target — the file whose bytes will
            // actually be replaced.
            let exists = crate::fsx::exists_via(&**fs, &resolved);
            match purpose {
                crate::file_browser::DestinationPurpose::SaveAs => {
                    if exists {
                        editor.pending_save_overwrite = Some(resolved.clone());
                        editor.pending_save_as_chosen = Some(chosen);
                        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&resolved));
                    } else {
                        crate::prompts::perform_save_as(
                            editor, chosen, resolved, executor, clock, msg_tx, fs);
                    }
                }
                crate::file_browser::DestinationPurpose::WriteBlock => {
                    let Some(b) = editor.active().marked_block else {
                        editor.set_status(crate::status::StatusKind::Info, "no marked block");
                        return;
                    };
                    if exists {
                        editor.pending_write_block = Some(resolved);
                        editor.open_prompt(crate::prompt::Prompt::write_block_overwrite(
                            editor.pending_write_block.as_ref().expect("just set")));
                    } else {
                        crate::prompts::perform_block_write(editor, &resolved, b.start, b.end, fs);
                    }
                }
                crate::file_browser::DestinationPurpose::Export { ext } => {
                    if exists {
                        editor.pending_export = Some(crate::export::PendingExport {
                            ext, target: resolved });
                        editor.open_prompt(crate::prompt::Prompt::export_overwrite(
                            &editor.pending_export.as_ref().expect("just set").target));
                    } else {
                        crate::export::do_export(editor, &ext, &resolved, msg_tx, false,
                            std::sync::Arc::clone(fs));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_browser::FileEntry;
    use crate::fsx::EntryKind;
    use crate::jobs::Executor;

    // The shared keystroke helpers (`press_key_fb`, `press_char_fb`, `press_enter_fb`,
    // `nix_privileged`) live in `test_support` as of Task 12. Only `press_key_fb` is used
    // in THIS module's tests (the Tab gesture) — `press_char_fb`/`press_enter_fb` are not
    // imported here to avoid an unused-import warning (a build-clean GATE); the other tests
    // in this file drive Enter/typing through pure `classify_destination_enter` calls or the
    // real mouse path instead.
    use crate::test_support::press_key_fb;

    fn fe(name: &str, kind: EntryKind) -> FileEntry {
        FileEntry { name: name.into(), kind, is_symlink: false, broken: false }
    }
    fn tmp(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "wc-commit-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), label));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    // ---- The four rows of the decision table, in order ----------------------------

    #[test]
    fn row1_highlighted_directory_descends() {
        let d = tmp("row1");
        std::fs::create_dir_all(d.join("drafts")).expect("seed");
        let e = fe("drafts", EntryKind::Dir);
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter", Some(&e), true),
            CommitOutcome::Descend(d.join("drafts")),
            "row 1 wins even with a non-empty field — the writer keeps their filename while \
             navigating");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row2_empty_field_on_a_highlighted_file_commits_to_it() {
        // Explicit overwrite of an existing file. Safe because it still raises the
        // overwrite-confirm downstream, and because reaching it takes TWO deliberate acts:
        // navigating the highlight there, and pressing Enter with a visibly empty field.
        let d = tmp("row2");
        std::fs::write(d.join("existing.md"), b"x").expect("seed");
        let e = fe("existing.md", EntryKind::File);
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "", Some(&e), false),
            CommitOutcome::Commit { path: d.join("existing.md"), from_highlight: true });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row3_field_naming_an_existing_directory_descends_not_creates() {
        // THE AMBIGUOUS CASE, resolved toward descend. A directory named `chapter-one`
        // sitting visibly in the list while Enter silently creates a FILE named
        // `chapter-one.md` beside it is the worse surprise — and descend is recoverable in
        // one keystroke ('..'), while a misplaced file is not.
        let d = tmp("row3");
        std::fs::create_dir_all(d.join("chapter-one")).expect("seed");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None, false),
            CommitOutcome::Descend(d.join("chapter-one")),
            "a field naming an existing directory descends into it");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row3_is_pinned_one_character_away_from_row4() {
        // The companion that PINS row 3: adding a character must flip it to file creation.
        // Without this, "resolves toward descend" could be satisfied by a rule that never
        // creates a file whose name shares a prefix with a directory.
        let d = tmp("row3-pin");
        std::fs::create_dir_all(d.join("chapter-one")).expect("seed");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None, false),
            CommitOutcome::Descend(d.join("chapter-one")));
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-oneX", None, false),
            CommitOutcome::Commit { path: d.join("chapter-oneX"), from_highlight: false },
            "one more character and it is an ordinary new-file commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row4_commits_dir_plus_field() {
        let d = tmp("row4");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter one", None, false),
            CommitOutcome::Commit { path: d.join("chapter one"), from_highlight: false },
            "the ordinary case: a new file in the directory the writer is looking at");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- Field resolution ---------------------------------------------------------

    #[test]
    fn a_bare_relative_field_resolves_against_fb_dir_not_the_process_cwd() {
        // The divergence from `prompts::expand_path`, and the whole point of it: the writer
        // is looking at `dir`, so `chapter.md` must mean "here". Joining cwd would put the
        // file somewhere the picker never showed them — the save-to-nowhere class.
        let d = tmp("resolve-rel");
        let cwd = std::env::current_dir().expect("cwd");
        assert_ne!(d, cwd, "test premise: fb.dir and cwd must differ");
        assert_eq!(resolve_field(&d, "chapter.md"), d.join("chapter.md"));
        assert_eq!(resolve_field(&d, "drafts/ch1.md"), d.join("drafts/ch1.md"),
            "a relative path WITH segments also resolves under fb.dir");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn absolute_and_home_relative_fields_are_honoured() {
        // The `~/` assertion is MANDATORY, not conditional. It was originally guarded by
        // `if let Some(home) = dirs::home_dir()`, which meant the entire tilde-expansion
        // branch could be missing and this test would still pass on any container without
        // a resolvable home — a vacuous pass exactly where the interesting behaviour is.
        // `dirs::home_dir()` reads $HOME on unix, so the test SETS it and owns the answer.
        //
        // FAIL-VERIFY: delete the `~/` arm from `resolve_field`, watch this fail.
        let d = tmp("resolve-abs");
        assert_eq!(resolve_field(&d, "/etc/hosts"), std::path::PathBuf::from("/etc/hosts"));

        let home = tmp("resolve-home");
        let prior = std::env::var_os("HOME");
        // Edition 2021: `set_var` is safe here (it becomes `unsafe` only in edition 2024).
        std::env::set_var("HOME", &home);
        let got = resolve_field(&d, "~/notes.md");
        match prior { Some(v) => std::env::set_var("HOME", v),
                      None    => std::env::remove_var("HOME") }
        assert_eq!(got, home.join("notes.md"),
            "`~/` expands against the home dir, unconditionally asserted");

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- The Tab gesture ----------------------------------------------------------

    #[test]
    fn tab_copies_a_name_into_the_field_and_does_not_commit() {
        // The deliberate two-step overwrite gesture: highlight, Tab (see the name land and
        // the footer show the resolved target), Enter (see the overwrite-confirm). Overwrite
        // is never one accidental keystroke, and never reachable without the target visible.
        // Driven through the REAL intercept. Calling `copy_name_into_field` directly proves
        // the helper works, not that Tab reaches it — and "does not commit" would rest on a
        // comment rather than an assertion.
        //
        // FAIL-VERIFY: remove the `KeyCode::Tab` arm from the destination branch, watch the
        // field assertion fail; wire Tab to `commit_destination`, watch the no-commit
        // assertions fail.
        let d = tmp("tab-gesture");
        std::fs::write(d.join("existing.md"), b"x").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.file_browser = Some(crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: "draft".into(), field_cursor: 5,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: vec![FileEntry { name: "existing.md".into(), kind: EntryKind::File,
                is_symlink: false, broken: false }],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        });

        press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Tab);

        match &e.file_browser.as_ref().expect("picker stays open").mode {
            crate::file_browser::BrowseMode::Destination { field, field_cursor, .. } => {
                assert_eq!(field, "existing.md", "Tab REPLACES the field content");
                assert_eq!(*field_cursor, "existing.md".len(), "cursor lands at the end");
            }
            other => panic!("still destination mode, got {other:?}"),
        }
        assert!(e.file_browser.is_some(), "Tab does NOT commit — the picker stays open");
        assert!(e.pending_save_overwrite.is_none(), "and raises no overwrite-confirm");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_click_on_a_file_in_destination_mode_copies_the_name_and_does_not_commit() {
        // THE CLICK DIVERGENCE (decision 9) — driven through the REAL mouse path.
        //
        // An earlier version called `click_commit_or_copy` directly. `mouse.rs` is currently
        // wired to `file_browser_enter`, so that test passed while a live click COMMITTED —
        // it guarded the safety property by asserting on a function the click never reached.
        //
        // FAIL-VERIFY: leave `mouse::mouse_file_browser`'s `Down(Left)` arm calling
        // `file_browser_enter` unconditionally (i.e. do not add the mode branch), watch this
        // fail — the picker closes and `victim.md` is overwritten.
        let d = tmp("click-divergence");
        std::fs::write(d.join("victim.md"), b"precious\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("draft\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Built from THIS task's own types — `open_destination_picker` is Task 21.
        e.file_browser = Some(crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: String::new(), field_cursor: 0,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: vec![FileEntry {
                name: "victim.md".into(), kind: EntryKind::File, is_symlink: false, broken: false }],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        });

        // A REAL left-click on the row the painter drew, routed through the overlay mouse
        // table exactly as `reduce` routes it.
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        // Row 0's cell, computed from the overlay rect directly. Task 20 adds
        // `chrome_geom::file_browser_row_origin`; using it here would be a forward reference,
        // and row 0 sits at `list_top` by construction so this needs no helper.
        let ov = crate::chrome_geom::palette_overlay_rect(area,
            e.file_browser.as_ref().expect("picker open").entries.len());
        let (col, row) = (ov.x + 1, ov.y + 2);   // +1 border column, +2 border + query row
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &fs };
        crate::mouse::mouse_file_browser(&mut e, crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col, row, modifiers: crossterm::event::KeyModifiers::NONE,
        }, area, &ctx);

        // The name landed in the FIELD…
        match &e.file_browser.as_ref().expect("picker stays open").mode {
            crate::file_browser::BrowseMode::Destination { field, .. } =>
                assert_eq!(field, "victim.md", "a click copies the name into the field"),
            other => panic!("still destination mode, got {other:?}"),
        }
        // …and NOTHING was written or dispatched.
        assert!(e.file_browser.is_some(), "the picker must NOT close — a click does not commit");
        assert!(e.pending_save_overwrite.is_none(),
            "and it must NOT raise the overwrite-confirm — that needs a deliberate Enter");
        assert_eq!(std::fs::read_to_string(d.join("victim.md")).expect("read"), "precious\n",
            "the file on disk is untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- All three drive Enter through the INTERCEPT, not commit_destination -----------
    //
    // FAIL-VERIFY (all three): delete the `KeyCode::Enter` arm from
    // `file_browser_intercept`'s destination branch, watch all three fail, then revert.
    //
    // An earlier draft called `commit_destination` directly. That is the vacuous-guard
    // pattern: a missing or mis-wired Enter arm would pass every one of them, and "the
    // commit path does not exist" is the exact defect the gate caught in this task last
    // round. A test named end-to-end that skips the entry point reads as coverage while
    // proving only that a function it hand-called works.

    fn press_enter(e: &mut crate::editor::Editor, fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        ex: &dyn crate::jobs::Executor, clk: &dyn wordcartel_core::history::Clock,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>)
    {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex, clock: clk, msg_tx: tx, fs };
        let enter = Event::Key(KeyEvent {
            code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let _ = crate::file_browser_intercept::intercept(crate::app::Msg::Input(enter), e, &ctx);
    }

    /// C5 review finding I1 — probe-proven against the branch, then fixed here. Row 2 (empty
    /// field, Enter on a highlighted existing file) piped its target through extension policy,
    /// whose `Defaulted` arm retargets an EXTENSIONLESS name: choosing an existing `README`
    /// wrote a brand-new `README.md` instead. `README` was untouched, no overwrite prompt was
    /// raised (the new path did not exist), and no footer disclosure appeared (the field was
    /// empty) — the writer's only clue was a status line naming a file they never chose.
    ///
    /// That contradicts Row 2's own contract ("commit to THAT file … goes through the
    /// overwrite-confirm prompt, which is what makes this safe") and §8's scoping of the
    /// policy to "a pure classification function over the FIELD TEXT" — a Row-2 commit has no
    /// field text. An emergent composition of three individually-correct tasks (T18's table,
    /// T19's policy, T21's wiring), which is why no single task owned it.
    ///
    /// Driven through the REAL intercept — arrow keys to navigate, Enter to commit — because
    /// the defect lives in the composition, not in any one function's return value.
    ///
    /// FAIL-VERIFY: drop the `_ if from_highlight => raw` arm from `commit_destination`, watch
    /// this fail with `README.md` created and no prompt. Confirmed, then reverted.
    #[test]
    fn row2_onto_an_extensionless_file_targets_that_exact_file() {
        let d = tmp("row2-extensionless");
        std::fs::write(d.join("README"), b"precious readme\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("buffer body\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), String::new());
        crate::test_support::pump_listing(&mut e, &rx);

        // Navigate DELIBERATELY onto `README` — Row 2 requires `highlight_is_navigated()`.
        for _ in 0..8 {
            let on_readme = e.file_browser.as_ref()
                .and_then(|fb| fb.entries.get(fb.selected))
                .is_some_and(|r| r.name == "README");
            if on_readme { break; }
            crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Down);
        }
        {
            let fb = e.file_browser.as_ref().expect("picker open");
            assert_eq!(fb.entries.get(fb.selected).map(|r| r.name.as_str()), Some("README"),
                "precondition: the highlight is on the existing extensionless file");
            assert!(fb.highlight_is_navigated(), "precondition: the writer moved it there");
            assert_eq!(fb.mode.filter_text(&fb.query), "",
                "precondition: the field is empty — this is Row 2, not Row 4");
        }

        crate::test_support::press_enter_fb(&mut e, &fs, &tx);

        assert!(!d.join("README.md").exists(),
            "extension policy must NOT retarget a highlighted existing file to a different one");
        assert_eq!(std::fs::read_to_string(d.join("README")).expect("still there"),
            "precious readme\n", "and nothing is written until the writer confirms");
        assert_eq!(e.pending_save_overwrite.as_deref(),
            Some(std::fs::canonicalize(d.join("README")).expect("canon").as_path()),
            "the overwrite-confirm — Row 2's whole safety argument — must actually be raised");
        assert!(e.prompt.is_some(), "and the writer must be looking at it");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_as_commits_end_to_end_from_enter() {
        let d = std::env::temp_dir().join(format!("wc-saveas-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let mut e = crate::editor::Editor::new_from_text("chapter body\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "chapter one".into());
        // Pump the async listing to completion — the state real usage actually reaches. Before
        // the parent-row-highlight fix this would have hit Row 1 on the default ".." highlight
        // and descended instead of committing; see the FAIL-VERIFY on the test below.
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // Extension policy applied, file written, buffer rekeyed and clean.
        assert_eq!(std::fs::read_to_string(d.join("chapter one.md")).expect("written"),
            "chapter body\n");
        assert_eq!(e.active().document.path.as_deref(), Some(d.join("chapter one.md").as_path()));
        assert!(!e.active().document.dirty());
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// REGRESSION — the parent-row-highlight defect. `filter_and_rank` pins the synthetic
    /// ".." row at `entries[0]` unconditionally whenever the picker is not at filesystem root,
    /// and `FileBrowser::selected` initializes to 0. So the moment the async listing lands —
    /// with NOTHING typed yet and NO navigation performed — ".." is already highlighted. Row 1
    /// of `classify_destination_enter` used to descend on ANY highlighted directory, even with
    /// a non-empty field (deliberately, so a writer can type-then-navigate-then-commit); the two
    /// combined so the ordinary "type a name, press Enter" sequence never reached Row 4
    /// (Commit) at all — it hit Row 1 (Descend) on a highlight the writer never touched.
    ///
    /// Fixed by gating Row 1 on `FileBrowser::highlight_is_navigated()` — true only once the
    /// writer has deliberately moved the highlight (arrow keys, a click, a wheel scroll) — OR an empty
    /// field. Reproduced BEFORE the fix (confirmed live, then reverted — see the task report):
    /// with the gate removed, this test's write assertions below fail and `pending_dir` instead
    /// shows a descend into `d.parent()`.
    ///
    /// This mirrors what a writer actually does: open the picker on a NON-ROOT directory
    /// (so ".." exists), pump the listing to completion (unlike the other Enter-through tests
    /// in this module, which deliberately seed via `open_destination_picker`'s `field` param
    /// and never populate `entries` from a real listing — see the comments on
    /// `redirect_clears_the_pending_quit_drain_state` / the trailing-separator test above,
    /// which name this exact gap), type a filename through the real intercept, and press Enter
    /// through the real intercept.
    ///
    /// FAIL-VERIFY: change Row 1's guard back to `matches!(e.kind, EntryKind::Dir)` (dropping
    /// `&& (highlight_navigated || trimmed.is_empty())`) in `classify_destination_enter`, watch
    /// this fail (no file written, `file_browser` still open, `pending_dir` == `d.parent()`),
    /// then restore.
    #[test]
    fn typing_a_name_after_the_listing_lands_commits_not_descends() {
        let d = tmp("parent-highlight-defect");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Opened with an EMPTY field — exactly what a writer sees invoking Save-As fresh.
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), String::new());
        // Pump the async listing to completion — the state real usage actually reaches,
        // unlike the hand-seeded `field` shortcuts elsewhere in this module.
        crate::test_support::pump_listing(&mut e, &rx);

        let fb = e.file_browser.as_ref().expect("picker open");
        assert_eq!(fb.entries.first().map(|r| r.name.as_str()), Some(".."),
            "precondition: the parent row is present (d is not filesystem root)");
        assert_eq!(fb.selected, 0,
            "precondition: nothing has been typed or navigated yet — the highlight still \
             defaults to row 0, which IS the '..' row");
        assert!(!fb.highlight_is_navigated(), "precondition: the writer has not touched the highlight");

        // Type a filename through the REAL intercept, one keystroke at a time — no navigation.
        for c in ['c', 'h', 'a', 'p'] {
            crate::test_support::press_char_fb(&mut e, &fs, &tx, c);
        }
        assert_eq!(e.file_browser.as_ref().expect("open").selected, 0,
            "typing does not move the highlight off row 0");

        // Enter through the REAL intercept, exactly as a writer's keystroke would arrive.
        press_enter(&mut e, &fs, &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // Enter with a typed name must COMMIT — write the file and close the picker — never
        // silently descend on a highlight the writer never touched.
        assert_eq!(std::fs::read_to_string(d.join("chap.md")).expect("written"), "body\n",
            "typing a name and pressing Enter must write the file, not navigate to the parent");
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The OTHER half of the fix: a genuinely navigated highlight must not disable Row 1. This
    /// is the design intent the task brief calls out as worth preserving: type a name,
    /// arrow-key into a subfolder, and commit there — keeping the typed name. Driven
    /// end-to-end through the real listing + real keyboard-nav + real Enter intercept, not by
    /// hand-constructing `highlighted`/`navigated_name` — so a regression in HOW the flag gets
    /// set (not just what it gates) would be caught here too.
    ///
    /// The subfolder is named `chapter-drafts` — deliberately containing the typed field text
    /// as a substring — so it SURVIVES the dual-duty field-as-filter (`rederive`'s fuzzy
    /// filter over `fb.mode.filter_text`) and is still on screen to arrow onto; an unrelated
    /// name like `drafts` would be filtered out by a field of `chapter` before the writer could
    /// ever navigate to it, which is a separate, pre-existing property of the dual-duty filter,
    /// not something this test is about.
    #[test]
    fn arrowing_to_a_real_directory_still_descends_and_keeps_the_typed_field() {
        let d = tmp("row1-preserved");
        std::fs::create_dir_all(d.join("chapter-drafts")).expect("seed dir");
        std::fs::write(d.join("chapter-notes.md"), b"x").expect("seed unrelated match");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Field pre-filled with "chapter" — equivalent to the writer having already typed it —
        // so the listing filters to just ".." plus the two `chapter*` entries once it lands.
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "chapter".into());
        crate::test_support::pump_listing(&mut e, &rx);

        let names: Vec<String> = e.file_browser.as_ref().expect("open")
            .entries.iter().map(|r| r.name.clone()).collect();
        let dir_idx = names.iter().position(|n| n == "chapter-drafts")
            .expect("chapter-drafts survives the field-as-filter — precondition");
        assert!(!e.file_browser.as_ref().unwrap().highlight_is_navigated(),
            "precondition: nothing has been navigated yet");

        // Arrow DOWN through the REAL intercept until "chapter-drafts" is highlighted.
        for _ in 0..dir_idx {
            crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Down);
        }
        let fb = e.file_browser.as_ref().expect("open");
        assert_eq!(fb.entries.get(fb.selected).map(|r| r.name.as_str()), Some("chapter-drafts"),
            "precondition: the real directory is now highlighted");
        assert!(fb.highlight_is_navigated(), "a real nav key sets the flag");

        // Enter through the REAL intercept — Row 1 must still fire: descend, not commit.
        crate::test_support::press_enter_fb(&mut e, &fs, &tx);

        let fb = e.file_browser.as_ref()
            .expect("the picker stays open — a descend never commits");
        assert_eq!(fb.pending_dir.as_deref(), Some(d.join("chapter-drafts").as_path()),
            "Enter on a navigated directory highlight still descends into it");
        match &fb.mode {
            crate::file_browser::BrowseMode::Destination { field, .. } =>
                assert_eq!(field, "chapter", "the typed field survives navigating — Row 1's point"),
            other => panic!("expected destination mode, got {other:?}"),
        }
        assert!(!d.join("chapter.md").exists(), "nothing was written — this was a descend");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// GAP 1 REGRESSION (re-review of the parent-row-highlight fix above). The fix's own
    /// intercept arm used to set `highlight_navigated = true` on ANY recognised nav key,
    /// whether or not `apply_list_nav` actually moved `selected` — so an ordinary reflex `Up`
    /// press at the TOP of the list (a genuine no-op: `saturating_sub` keeps `selected` at 0)
    /// still armed Row 1. That reproduces the EXACT symptom the fix above closes: with the
    /// highlight still sitting untouched on ".." at row 0, typing a filename and pressing
    /// Enter descended into the parent instead of committing.
    ///
    /// Fixed by only stamping `FileBrowser::navigated_name` when `selected` actually changed
    /// value (`file_browser_intercept.rs`'s nav arm now compares before/after
    /// `apply_list_nav`) — see `FileBrowser::navigated_name`'s doc comment for why name
    /// tracking ALONE would not have been enough: a no-op key would just re-record the same
    /// name already highlighted.
    ///
    /// FAIL-VERIFY: revert the nav-key arm in `file_browser_intercept.rs` to stamp
    /// `fb.navigated_name` unconditionally on any recognised key (dropping the
    /// `if fb.selected != before` guard), watch this fail — `chap.md` is never written, the
    /// picker stays open, `pending_dir` shows a descend into `d.parent()` — then restore.
    #[test]
    fn a_noop_nav_key_does_not_arm_row1_so_typing_then_enter_still_commits() {
        let d = tmp("gap1-noop-nav");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), String::new());
        crate::test_support::pump_listing(&mut e, &rx);

        let fb = e.file_browser.as_ref().expect("picker open");
        assert_eq!(fb.selected, 0, "precondition: default highlight sits on row 0 (the '..' row)");
        assert!(!fb.highlight_is_navigated(), "precondition: nothing chosen yet");

        // An ordinary reflex Up keypress at the top of the list. `list_window::apply_list_nav`'s
        // Up arm is `selected.saturating_sub(1)` — 0 stays 0. Nothing moved.
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Up);
        let fb = e.file_browser.as_ref().expect("picker open");
        assert_eq!(fb.selected, 0, "precondition: Up at the top is a genuine no-op");
        assert!(!fb.highlight_is_navigated(),
            "a no-op nav key must NOT arm Row 1 — the exact defect this test guards");

        // Type a filename through the REAL intercept, one keystroke at a time — no navigation.
        for c in ['c', 'h', 'a', 'p'] { crate::test_support::press_char_fb(&mut e, &fs, &tx, c); }

        // Enter through the REAL intercept, exactly as a writer's keystroke would arrive.
        press_enter(&mut e, &fs, &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert_eq!(std::fs::read_to_string(d.join("chap.md")).expect("written"), "body\n",
            "typing a name and pressing Enter must WRITE, not descend into the parent");
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// GAP 2 REGRESSION (re-review of the parent-row-highlight fix above). `rederive` clamps
    /// `fb.selected` when it is out of bounds after a re-filter shrinks `entries`, but the OLD
    /// code never re-validated that the entry AT that index was still the one the writer
    /// chose — so a stale `highlight_navigated = true` could survive onto a DIFFERENT entry
    /// that merely slid into the same slot.
    ///
    /// Reproduced exactly as the task describes: seed `cab` and `cat` (both dirs), filter to
    /// "c" (both survive), arrow down onto `cab` (arming the flag against `cab` specifically),
    /// then type `t` — the field becomes "ct", which filters `cab` OUT (it has no 't') while
    /// `cat` survives. With only `cab`/`cat` as candidates, whatever `fb.selected` resolves to
    /// after the shrink (whether via the out-of-bounds clamp or a plain index shift) MUST now
    /// be `cat` — an entry the writer never touched.
    ///
    /// Fixed by storing the CHOSEN entry's NAME (`FileBrowser::navigated_name`) and
    /// re-comparing it against `entries[selected]` live, on every read
    /// (`FileBrowser::highlight_is_navigated`), rather than trusting a bare bool set once and
    /// never revisited — see that method's doc comment.
    ///
    /// FAIL-VERIFY: make `highlight_is_navigated` return `self.navigated_name.is_some()`
    /// without comparing it against `entries[self.selected]`'s name (i.e. trust the stale
    /// bool-shaped fact alone), watch this fail — `ct.md` is never written, the picker stays
    /// open, `pending_dir` shows a descend into `cat` — then restore.
    #[test]
    fn navigating_onto_an_entry_that_then_filters_out_does_not_leave_a_stale_flag_on_whatever_slides_in() {
        let d = tmp("gap2-stale-flag");
        std::fs::create_dir_all(d.join("cab")).expect("seed cab");
        std::fs::create_dir_all(d.join("cat")).expect("seed cat");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "c".into());
        crate::test_support::pump_listing(&mut e, &rx);

        let names: Vec<String> = e.file_browser.as_ref().expect("open")
            .entries.iter().map(|r| r.name.clone()).collect();
        let cab_idx = names.iter().position(|n| n == "cab")
            .expect("cab survives filter 'c' — precondition");
        assert!(names.iter().any(|n| n == "cat"), "cat also survives filter 'c': {names:?}");

        // Arrow DOWN onto "cab" through the REAL intercept.
        for _ in 0..cab_idx {
            crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Down);
        }
        let fb = e.file_browser.as_ref().expect("open");
        assert_eq!(fb.entries.get(fb.selected).map(|r| r.name.as_str()), Some("cab"),
            "precondition: 'cab' is highlighted");
        assert!(fb.highlight_is_navigated(), "precondition: the flag is armed against 'cab'");

        // Type 't' — field becomes "ct". "cab" has no 't' at all, so it filters OUT; "cat"
        // (c…a…t) survives the fuzzy subsequence match. Only ".." and "cat" remain.
        crate::test_support::press_char_fb(&mut e, &fs, &tx, 't');
        let fb = e.file_browser.as_ref().expect("open");
        let names: Vec<String> = fb.entries.iter().map(|r| r.name.clone()).collect();
        assert!(!names.iter().any(|n| n == "cab"), "'cab' must have filtered out: {names:?}");
        assert_eq!(fb.entries.get(fb.selected).map(|r| r.name.as_str()), Some("cat"),
            "precondition: 'cat' now sits wherever 'cab' used to (shift or clamp): {names:?}");
        assert!(!fb.highlight_is_navigated(),
            "the flag must NOT survive onto 'cat' — the writer only ever chose 'cab'");

        // Enter through the REAL intercept: "ct" names no existing path, so with the flag
        // correctly cleared this must fall through to Row 4 and COMMIT — never Row 1's stale
        // descend into 'cat'.
        press_enter(&mut e, &fs, &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert_eq!(std::fs::read_to_string(d.join("ct.md")).expect("written"), "body\n",
            "Enter must WRITE — a stale flag must not descend into 'cat' instead");
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// PAIRED-LIFETIME regression: `pending_save_as_chosen` must be cleared everywhere
    /// `pending_save_overwrite` is abandoned, or a stale `chosen` from THIS round trip could
    /// pair with a different `resolved` on a LATER one — a silent wrong-target write. Commit
    /// onto an EXISTING target (raising the OverwriteSaveAs modal, which sets both fields),
    /// then cancel it — driven through the real modal intercept, not by inspecting the two
    /// fields' setters directly.
    ///
    /// FAIL-VERIFY: drop the `pending_save_as_chosen = None` line from `prompts::intercept`'s
    /// Esc arm, watch the `chosen_survives` assertion fail, then restore it.
    #[test]
    fn cancelling_the_overwrite_modal_clears_both_paired_fields() {
        let d = std::env::temp_dir().join(format!("wc-saveas-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("taken.md"), b"already here\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("new body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "taken.md".into());
        // Pump the async listing to completion — the state real usage actually reaches.
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);
        assert!(e.pending_save_overwrite.is_some(), "existing target raises the overwrite modal");
        assert!(e.pending_save_as_chosen.is_some(), "and pairs it with the chosen path");
        assert!(e.prompt.is_some());

        // Esc on the modal, through `prompts::intercept` — the real path, not a direct field
        // clear (a direct clear would pass whether or not the Esc arm's own cleanup exists).
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &fs };
        let esc = crossterm::event::Event::Key(crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Esc, modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE });
        crate::prompts::intercept(crate::app::Msg::Input(esc), &mut e, &ctx);

        assert!(e.prompt.is_none(), "Esc dismisses the modal");
        assert!(e.pending_save_overwrite.is_none(), "resolved half cleared");
        let chosen_survives = e.pending_save_as_chosen.is_some();
        assert!(!chosen_survives,
            "chosen half must ALSO be cleared — a surviving chosen could pair with a \
             DIFFERENT resolved on a later round trip and silently write to the wrong target");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// CRITICAL-1 regression: the `Redirect` arm used to leave `pending_save_as` and
    /// `quit_drain` armed while it reopened an Export picker. A `Redirect` IS an abandoned
    /// save — the write never happened, and the writer is being offered a different feature
    /// — so it must clear the same fields the `CommitOutcome::Nothing` empty-path arm does.
    /// Without the fix, a save that later completes on some OTHER target could `.take()`
    /// this stale `Quit` and fire it — the writer never asked to quit anymore.
    ///
    /// Armed exactly as the reviewer reproduced it live: `pending_save_as = Some(Quit)`
    /// paired with an armed `quit_drain` (the state `dispatch_save_then` sets for
    /// save-and-quit on an unnamed buffer). Seeded exactly as the other Enter-through
    /// tests in this module seed a typed name — `open_destination_picker`'s `field`
    /// parameter.
    ///
    /// Now PUMPS the real listing before the Enter (the parent-row-highlight fix, task
    /// report). Previously this deliberately skipped pumping: typing/pumping populated
    /// `fb.entries` with a ".." row unconditionally pinned at index 0 (`filter_and_rank`),
    /// which `classify_destination_enter`'s Row 1 would act on regardless of the non-empty
    /// field, hitting Descend instead of the Row-4 Commit this test needs to drive the
    /// Redirect arm. `FileBrowser::highlight_is_navigated()` now gates Row 1 on the writer
    /// having actually moved the highlight (or an empty field), so pumping a real listing here
    /// is safe and exercises the state real usage reaches. Commit itself is still driven
    /// through the REAL Enter intercept.
    ///
    /// FAIL-VERIFY: remove the clearing added to the `Redirect` arm, watch the
    /// `pending_save_as`/`quit_drain` assertions fail, then restore.
    #[test]
    fn redirect_clears_the_pending_quit_drain_state() {
        let d = tmp("redirect-quit-strand");
        let mut e = crate::editor::Editor::new_from_text("unsaved\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "notes.html".into());
        crate::test_support::pump_listing(&mut e, &rx);
        e.pending_save_as = Some(crate::editor::PostSaveAction::Quit);
        e.quit_drain = Some(crate::editor::QuitDrain {
            queue: std::collections::VecDeque::new(), mode: crate::editor::QuitMode::SaveAll });

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        assert!(e.pending_save_as.is_none(),
            "a Redirect abandons the save — the armed post-save action must not survive");
        assert!(e.quit_drain.is_none(), "and the drain must not be left stranded");
        assert!(!e.quit_drain_advance);
        assert!(!e.quit, "no stale Quit can fire from a save that never happened");

        // And the redirect itself still did its job: an Export picker opened for the
        // typed name, carrying the writer's intent forward.
        match &e.file_browser.as_ref().expect("export picker opened").mode {
            crate::file_browser::BrowseMode::Destination { purpose, field, .. } => {
                assert_eq!(purpose,
                    &crate::file_browser::DestinationPurpose::Export { ext: "html".into() });
                assert_eq!(field, "notes.html");
            }
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// CRITICAL-2 regression: `resolve_prompt`'s `OverwriteSaveAs` arm calls
    /// `perform_save_as(editor, chosen, resolved, …)`. Swap those two positional
    /// `PathBuf` arguments and every existing test still passed — the two existing
    /// overwrite tests (above) only assert that the modal RAISES and that Esc clears
    /// both fields; neither drives the CONFIRM keypress through to an actual write.
    ///
    /// A symlink makes `chosen` and `resolved` genuinely different paths so a swap is
    /// observable: bytes must land at the RESOLVED (canonical) target, and
    /// `document.path` must end up holding the CHOSEN (symlink) path.
    ///
    /// FAIL-VERIFY: swap the two arguments in the `OverwriteSaveAs` arm, watch the
    /// `real`-contents / `document.path` assertions fail, then restore.
    #[cfg(unix)]
    #[test]
    fn confirming_the_overwrite_modal_writes_to_the_resolved_symlink_target() {
        let d = tmp("overwrite-confirm");
        let real = d.join("real.md");
        let link = d.join("link.md");
        std::fs::write(&real, b"old\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let mut e = crate::editor::Editor::new_from_text("new body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            d.clone(), link.to_str().expect("utf8").to_string());
        // Pump the async listing to completion — the state real usage actually reaches.
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "the existing target through the symlink raises the overwrite modal");
        let resolved_seen = e.pending_save_overwrite.clone().expect("resolved half armed");
        assert_eq!(resolved_seen, std::fs::canonicalize(&real).expect("canonicalize"),
            "resolved must be the canonical target, not the symlink");
        assert_eq!(e.pending_save_as_chosen.as_deref(), Some(link.as_path()),
            "chosen half is the symlink the writer typed");

        // Confirm ('o'), through the REAL modal intercept — not `resolve_prompt` called
        // directly, so the key mapping AND the resolver are both exercised.
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &fs };
        let o = crossterm::event::Event::Key(crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Char('o'), modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE });
        crate::prompts::intercept(crate::app::Msg::Input(o), &mut e, &ctx);
        for out in ex.drain() { crate::jobs_apply::apply_outcome(out, &mut e); }

        assert_eq!(std::fs::read_to_string(&real).expect("read"), "new body\n",
            "the write must land at the RESOLVED target — a swapped argument would write \
             to (or through) the symlink path string instead");
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "the symlink itself must survive — atomic_replace renames over the TARGET");
        assert_eq!(e.active().document.path.as_deref(), Some(link.as_path()),
            "document.path must hold the CHOSEN path (the symlink), not the resolved target");
        assert!(!e.active().document.dirty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// IMPORTANT-3 regression: the `Refused` arm's early return/Warning is the only thing
    /// stopping a trailing-separator destination (which names a directory, not a file) from
    /// falling through and writing using the raw path. The pure `apply_extension_policy`
    /// unit test (below) pins the VERDICT, but nothing previously proved the arm is reachable
    /// — and actually short-circuits — on the live commit path.
    ///
    /// Seeded via `open_destination_picker`'s `field` parameter, same as the module's other
    /// Enter-through tests.
    ///
    /// Now PUMPS the real listing before the Enter (the parent-row-highlight fix, task
    /// report). Previously this deliberately avoided typing/pumping: it would have populated
    /// `fb.entries` with a ".." row unconditionally pinned at index 0 (`filter_and_rank`),
    /// which `classify_destination_enter`'s Row 1 would act on regardless of the non-empty
    /// field, hitting Descend before the extension policy ever ran. `FileBrowser::
    /// highlight_is_navigated()` now gates Row 1 on the writer having actually moved the
    /// highlight (or an empty field), so pumping a real listing here is safe. Commit itself is
    /// still driven through the REAL Enter intercept.
    ///
    /// FAIL-VERIFY: remove the `Refused` arm's early return, watch this fail (the picker
    /// closes and something lands on disk under `d`), then restore.
    #[test]
    fn a_trailing_separator_destination_is_refused_end_to_end_writes_nothing() {
        let d = tmp("refused-e2e");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "sub/".into());
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        assert!(e.file_browser.is_some(), "the picker stays open — a refusal is not a commit");
        assert!(e.prompt.is_none(), "no overwrite modal raised — nothing was resolved to a write");
        assert_eq!(e.status().map(crate::status::Status::kind),
            Some(crate::status::StatusKind::Warning), "a clear status explains the refusal");
        assert!(std::fs::read_dir(&d).expect("read dir").next().is_none(),
            "the directory stays completely empty — no `sub` dir, and no hidden `sub/.md` file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn write_block_commits_end_to_end_from_enter() {
        let d = std::env::temp_dir().join(format!("wc-wb-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let mut e = crate::editor::Editor::new_from_text("alpha beta gamma\n", None, (80, 24));
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock, d.clone(), "excerpt".into());
        // Pump the async listing to completion — the state real usage actually reaches.
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        assert_eq!(std::fs::read_to_string(d.join("excerpt.md")).expect("written"), "alpha");
        assert!(e.active().document.path.is_none(),
            "write-block does NOT rekey the buffer — it exports a slice");
        assert!(e.active().marked_block.is_some(), "block stays after write");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_commits_end_to_end_from_enter_through() {
        // Decision 4: a bare Enter on the PRE-SEEDED picker must reproduce today's
        // zero-decision export. Export had no Enter-through commit test at all.
        let d = std::env::temp_dir().join(format!("wc-exp-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src), (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Seeded exactly as `run_export` seeds it — the Enter-through path.
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::Export { ext: "html".into() },
            d.clone(), "notes.html".into());
        // Pump the async listing to completion — the state real usage actually reaches.
        // `d` also contains the source file `notes.md`, so the SAME `rx` used for the
        // ExportDone read below first drains exactly this one ListingDone.
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        // The commit arm dispatched an export for the seeded target. Assert on the DISPATCH
        // rather than on pandoc's output: pandoc may be absent on the gate machine, and the
        // wiring is what this test owns.
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        // Assert the DISPATCH specifically — an `|| status contains "export"` fallback would
        // pass on any export-ish status message, including a failure, proving nothing about
        // whether Enter reached the commit arm.
        // BOUNDED RECEIVE, not `try_iter()`: `do_export` spawns a thread, so an immediate
        // drain races it and the test would pass or fail on scheduling. Same discipline the
        // listing tests use.
        let dispatched = std::iter::from_fn(|| rx.recv_timeout(
                std::time::Duration::from_secs(5)).ok())
            .take(4)
            .any(|m| matches!(m,
                crate::app::Msg::ExportDone { ref target, .. } if target == &d.join("notes.html")));
        assert!(dispatched,
            "Enter on the pre-seeded picker must dispatch an ExportDone for notes.html \
             (status was {:?})", e.status_text());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_enter_through_reproduces_run_export_with_probe_derivation() {
        // The gap this closes: `export.rs`'s own tests call `run_export_with_probe` but
        // never press Enter, and the sibling test above presses Enter but seeds the picker
        // by hand-calling `open_destination_picker` with LITERALS ("notes.html", `d`) that
        // merely happen to match what `run_export` derives — it never calls `run_export` at
        // all. A reviewer proved the gap live: mutating `run_export_with_probe` to seed
        // `derived.file_stem()` instead of `derived.file_name()` (dropping the extension)
        // failed `export::`'s tests but left the sibling test above fully green. This test
        // drives the WHOLE chain — real derivation, real async listing pump, real Enter
        // intercept — through the actual production entry point, with the expected value
        // COMPUTED from `derived_export_path` rather than typed twice, so it cannot pass by
        // coincidence of two literals agreeing.
        let d = std::env::temp_dir().join(format!("wc-exp-seam-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src.clone()), (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);

        // Seed through the REAL production entry point — `run_export`'s own derivation is
        // what gets exercised here, not a hand-typed stand-in for it.
        crate::export::run_export_with_probe(&mut e, &fs, "html", &tx, || true);
        assert!(e.file_browser.is_some(), "run_export_with_probe must open the destination picker");

        // Pump the async listing to completion — a bare Enter without pumping would exercise
        // a highlight/state real usage never reaches (the same class of bug the parent-row
        // highlight fix upstream of this file addressed for save-as).
        crate::test_support::pump_listing(&mut e, &rx);

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        // Computed, never typed: the whole point of this test is that it cannot pass by two
        // literals coincidentally agreeing.
        let expected = crate::export::derived_export_path(&src, "html");

        assert!(e.file_browser.is_none(), "the picker closed on commit");
        // Assert on the DISPATCHED target rather than the file on disk. Draining `ExportDone`
        // through `apply_export_done` and checking the written bytes would make this test's
        // pass/fail depend on pandoc actually being installed on whatever machine runs the
        // gate (see `export_destination_picker_opens_without_pandoc_installed` and the sibling
        // `export_commits_end_to_end_from_enter_through` above, which apply the same
        // discipline for the same reason) — an environment assumption, not a code assertion.
        // The dispatched target is exactly what `run_export`'s derivation is responsible for,
        // and it is observable regardless of pandoc's presence.
        let dispatched_target = std::iter::from_fn(|| rx.recv_timeout(
                std::time::Duration::from_secs(5)).ok())
            .take(4)
            .find_map(|m| match m {
                crate::app::Msg::ExportDone { target, .. } => Some(target),
                _ => None,
            });
        assert_eq!(dispatched_target, Some(expected),
            "Enter on the picker seeded by run_export_with_probe must dispatch ExportDone \
             for exactly derived_export_path's target (status was {:?})", e.status_text());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_field_with_no_highlight_commits_nothing() {
        let d = tmp("nothing");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "", None, false),
            CommitOutcome::Nothing, "no field, no highlight — Enter is inert, never a write");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "   ", None, false),
            CommitOutcome::Nothing, "a whitespace-only field is empty");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- The extension policy (F4 default-and-redirect) ---------------------------

    #[test]
    fn extension_policy_table() {
        use std::path::PathBuf;
        let p = |s: &str| PathBuf::from(s);

        // Missing extension -> append .md.
        assert_eq!(apply_extension_policy(&p("/d/chapter one")),
            ExtVerdict::Defaulted(p("/d/chapter one.md")));

        // Recognized OUTPUT extensions -> redirect to Export, carrying the path.
        for ext in ["docx", "pdf", "html", "tex"] {
            assert_eq!(apply_extension_policy(&p(&format!("/d/book.{ext}"))),
                ExtVerdict::Redirect { path: p(&format!("/d/book.{ext}")), ext: ext.into() },
                "a save into an export format is refused and redirected, not written as markdown");
        }
        // Case-insensitive.
        assert_eq!(apply_extension_policy(&p("/d/book.DOCX")),
            ExtVerdict::Redirect { path: p("/d/book.DOCX"), ext: "docx".into() });

        // Anything else -> honoured silently.
        for name in ["notes.txt", "notes.rst", "notes.org", "notes.md"] {
            assert_eq!(apply_extension_policy(&p(&format!("/d/{name}"))),
                ExtVerdict::Honoured(p(&format!("/d/{name}"))));
        }

        // EDGE CASES, each a real way to get this wrong:
        // A dotfile's leading dot is NOT an extension — never produce `.gitignore.md`.
        assert_eq!(apply_extension_policy(&p("/d/.gitignore")),
            ExtVerdict::Honoured(p("/d/.gitignore")));
        assert_eq!(apply_extension_policy(&p("/d/.wordcartel.toml")),
            ExtVerdict::Honoured(p("/d/.wordcartel.toml")));
        // A trailing dot is no extension — and must not yield `notes..md`.
        assert_eq!(apply_extension_policy(&p("/d/notes.")),
            ExtVerdict::Defaulted(p("/d/notes.md")));
        // Only the FINAL component is the extension.
        assert_eq!(apply_extension_policy(&p("/d/chapter.one.md")),
            ExtVerdict::Honoured(p("/d/chapter.one.md")));
        assert_eq!(apply_extension_policy(&p("/d/chapter.one")),
            ExtVerdict::Honoured(p("/d/chapter.one")),
            "`one` is an unrecognized extension — honoured, not defaulted");

        // A TRAILING SEPARATOR names a directory, not a file — refuse rather than create a
        // hidden `sub/.md`.
        //
        // FAIL-VERIFY: remove the trailing-separator arm from `apply_extension_policy`,
        // watch this assert `ExtVerdict::Defaulted(p("/d/sub/.md"))` instead — confirmed,
        // then restored.
        assert_eq!(apply_extension_policy(&p("/d/sub/")),
            ExtVerdict::Refused(p("/d/sub/")),
            "a trailing separator names a directory — refuse, do not default a hidden .md file");

        // Four edge cases the review found handled-but-untested — each pinned so a future
        // change can't silently flip one without a test noticing.

        // `.foo.` — a dotfile AND a trailing dot. The dotfile guard short-circuits before the
        // trailing-dot filter ever runs, so it is honoured VERBATIM, trailing dot and all.
        assert_eq!(apply_extension_policy(&p("/d/.foo.")),
            ExtVerdict::Honoured(p("/d/.foo.")));

        // `.md` alone — `file_name()` is `.md`, which `Path::extension()` treats as a
        // dotfile (no OTHER dot), not an extension. Honoured, not re-defaulted to `.md.md`.
        assert_eq!(apply_extension_policy(&p("/d/.md")),
            ExtVerdict::Honoured(p("/d/.md")));

        // `.config.md` — a leading dot AND a real extension. The second dot means
        // `Path::extension()` returns `Some("md")` directly, so the dotfile guard is never
        // reached; `md` is not an OUTPUT ext, so it is honoured.
        assert_eq!(apply_extension_policy(&p("/d/.config.md")),
            ExtVerdict::Honoured(p("/d/.config.md")));

        // Empty path — contract only, not a special case: `classify_destination_enter`
        // guards an empty/whitespace-only field before Row 4 ever calls this, so an empty
        // `Path` is almost certainly unreachable in practice; pinned here so a change to
        // that guarantee doesn't silently regress this fallback.
        assert_eq!(apply_extension_policy(&p("")), ExtVerdict::Defaulted(p(".md")));
    }
}
