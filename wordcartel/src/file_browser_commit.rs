//! Destination-mode commit semantics: what Enter MEANS when the writer is naming a file.
//!
//! Split from `file_browser.rs` on one axis of change. This is the highest-risk logic in
//! C5 — the only place where an error produces silent overwrite or save-to-nowhere — so it
//! lives alone, is pure, and is tested row by row.

use crate::file_browser::FileEntry;
use crate::fsx::{EntryKind, Fs};
use std::path::{Path, PathBuf};

#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode Enter/commit path; forward reference
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Descend(PathBuf),
    Commit(PathBuf),
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
#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode commit path; forward reference
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
/// | # | Condition                                   | Action              |
/// |---|---------------------------------------------|---------------------|
/// | 1 | highlighted entry is a directory (incl "..")| Descend             |
/// | 2 | field empty AND highlighted entry is a file | Commit to that file |
/// | 3 | field resolves to an EXISTING directory     | Descend into it     |
/// | 4 | otherwise                                   | Commit dir + field  |
#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode Enter path; forward reference
pub(crate) fn classify_destination_enter(
    fs: &dyn Fs,
    dir: &Path,
    field: &str,
    highlighted: Option<&FileEntry>,
) -> CommitOutcome {
    // Row 1 — a highlighted directory descends, EVEN with a non-empty field, so the writer
    // keeps their filename while navigating.
    if let Some(e) = highlighted {
        if matches!(e.kind, EntryKind::Dir) {
            let target = if e.name == ".." {
                dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf())
            } else {
                dir.join(&e.name)
            };
            return CommitOutcome::Descend(target);
        }
    }

    let trimmed = field.trim();

    // Row 2 — an empty field commits onto the highlighted FILE. Explicit overwrite intent:
    // it takes navigating there AND pressing Enter with a visibly empty field, and it still
    // raises the overwrite-confirm downstream.
    if trimmed.is_empty() {
        return match highlighted {
            Some(e) if matches!(e.kind, EntryKind::File) => {
                CommitOutcome::Commit(dir.join(&e.name))
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
    CommitOutcome::Commit(resolved)
}

/// Extensions that mean "this is an export, not a save".
#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode commit path; forward reference
const OUTPUT_EXTS: &[&str] = &["docx", "pdf", "html", "tex"];

/// The verdict `apply_extension_policy` reaches for a SAVE destination's extension.
#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode commit path; forward reference
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtVerdict {
    /// Append `.md` — the name had no extension.
    Defaulted(PathBuf),
    /// A recognized OUTPUT extension. Refuse the save and offer Export, carrying the typed
    /// path forward so the writer's intent is not thrown away.
    Redirect { path: PathBuf, ext: String },
    /// Any other extension — honoured silently.
    Honoured(PathBuf),
}

/// F4's default-and-redirect policy for SAVE destinations.
///
/// Redirect is only defensible because export now HAS a destination (spec §9) — before C5,
/// "use Export instead" was advice with nowhere to go.
///
/// Never applied in select mode, and never to an export destination (whose extension is
/// fixed by the format).
#[allow(dead_code)] // C5 Task 21 wires this into the destination-mode commit path; forward reference
pub(crate) fn apply_extension_policy(path: &Path) -> ExtVerdict {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_browser::FileEntry;
    use crate::fsx::EntryKind;

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
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter", Some(&e)),
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
            classify_destination_enter(&crate::fsx::RealFs, &d, "", Some(&e)),
            CommitOutcome::Commit(d.join("existing.md")));
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
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None),
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
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None),
            CommitOutcome::Descend(d.join("chapter-one")));
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-oneX", None),
            CommitOutcome::Commit(d.join("chapter-oneX")),
            "one more character and it is an ordinary new-file commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row4_commits_dir_plus_field() {
        let d = tmp("row4");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter one", None),
            CommitOutcome::Commit(d.join("chapter one")),
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
            awaiting_epoch: 0, pending_dir: None,
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
            awaiting_epoch: 0, pending_dir: None,
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

    #[test]
    fn an_empty_field_with_no_highlight_commits_nothing() {
        let d = tmp("nothing");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "", None),
            CommitOutcome::Nothing, "no field, no highlight — Enter is inert, never a write");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "   ", None),
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
    }
}
