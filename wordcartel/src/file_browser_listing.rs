//! Pure listing pipeline for the file browser: cache -> filter -> rank -> disclosure.
//!
//! NO IO at all, and no `Editor`. Kept separate from `file_browser.rs` on one axis of
//! change: this module answers "which rows exist", not "what a browser is". The one-time
//! fetch that used to live here (`refetch`) moved off-thread (Task 13): `file_browser::
//! start_listing` spawns it, `file_browser::apply_listing_done` merges the result and
//! calls `rederive` below.

use crate::config::FileTypeFilter;
use crate::file_browser::{FileBrowser, FileEntry};
use crate::fsx::{DirEntryInfo, EntryKind};

/// VCS/system directory names withheld as clutter even though they are already
/// dot-prefixed — so the list stays honest if the dotfile rule ever changes.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn", ".jj", ".pijul"];

/// Extensions `file::open` can actually open. Deliberately EXCLUDES .docx/.pdf: there is
/// no import path and `file::open` refuses them as `OpenError::Binary`, so listing them in
/// select mode would build a select-then-error dead end.
const TEXT_EXTS: &[&str] = &["md", "markdown", "txt", "rst", "text"];

/// Output-format siblings, shown in DESTINATION mode only — there they are exactly the
/// files a writer needs to see in order not to clobber them.
const OUTPUT_EXTS: &[&str] = &["docx", "pdf", "html", "tex"];

/// Which entries `filter_and_rank` retains, and under which policy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FilterOpts {
    pub show_clutter: bool,
    pub types: FileTypeFilter,
    /// Destination mode also shows output-format siblings (.docx/.pdf/.html/.tex) so a
    /// writer can see what they might clobber. Select mode does not — there is no import
    /// path, and listing them would build a select-then-error dead end.
    pub destination: bool,
}

/// Everything the footer needs. `shown + hidden_clutter + hidden_type + capped_out +
/// unreadable == total_seen` — asserted by test, because §7.4's law is that the picker
/// never silently withholds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Disclosure {
    pub shown: usize,
    pub hidden_clutter: usize,
    pub hidden_type: usize,
    pub capped_out: usize,
    pub unreadable: usize,
    pub total_seen: usize,
}

/// Dotfiles plus VCS/system directory names. NO gitignore semantics (decision 2): they
/// carry near-zero value for this audience and a real hazard — a manuscript under an
/// aggressive ignore file would vanish.
pub(crate) fn is_clutter(name: &str) -> bool {
    name.starts_with('.') || VCS_DIRS.contains(&name)
}

/// Is this name a "document" for the type filter? `destination` widens the set to include
/// output-format siblings.
pub(crate) fn is_document(name: &str, destination: bool) -> bool {
    match std::path::Path::new(name).extension().and_then(|e| e.to_str()) {
        None => true, // extensionless files are plausibly prose
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            TEXT_EXTS.contains(&ext.as_str())
                || (destination && OUTPUT_EXTS.contains(&ext.as_str()))
        }
    }
}

/// Pure: `listing` -> (rows, disclosure). No IO, no Editor. `at_root` suppresses "..".
pub(crate) fn filter_and_rank(
    listing: &[DirEntryInfo],
    at_root: bool,
    query: &str,
    opts: FilterOpts,
    total_seen: usize,
    unreadable: usize,
) -> (Vec<FileEntry>, Disclosure) {
    let mut hidden_clutter = 0usize;
    let mut hidden_type = 0usize;
    let mut kept: Vec<&DirEntryInfo> = Vec::new();

    for e in listing {
        if !opts.show_clutter && is_clutter(&e.name) {
            hidden_clutter += 1;
            continue;
        }
        // Directories are NEVER withheld by the type filter — a filter that hides the path
        // to your file is a filter that lies. Broken links are never withheld either:
        // hiding one leaves the writer unable to see why their file appears missing.
        let type_exempt = matches!(e.kind, EntryKind::Dir) || e.broken;
        if !type_exempt
            && matches!(opts.types, FileTypeFilter::Documents)
            && !is_document(&e.name, opts.destination)
        {
            hidden_type += 1;
            continue;
        }
        kept.push(e);
    }

    let shown = kept.len();
    let capped_out = total_seen.saturating_sub(listing.len()).saturating_sub(unreadable);

    // Rank: fuzzy when a query is present (matching the palette and outline), otherwise
    // dirs-then-files alphabetical. `..` is pinned first and is NOT a listing entry.
    let mut rows: Vec<FileEntry> = Vec::new();
    if !at_root {
        rows.push(FileEntry {
            name: "..".into(), kind: EntryKind::Dir, is_symlink: false, broken: false,
        });
    }
    let mut ordered: Vec<DirEntryInfo> = kept.into_iter().cloned().collect();
    if query.is_empty() {
        ordered.sort_by(|a, b| {
            let ad = matches!(a.kind, EntryKind::Dir);
            let bd = matches!(b.kind, EntryKind::Dir);
            bd.cmp(&ad).then_with(|| a.name.cmp(&b.name))
        });
    } else {
        ordered = crate::palette::fuzzy_filter(&ordered, query, |e| e.name.as_str());
    }
    rows.extend(ordered.into_iter().map(|e| FileEntry {
        name: e.name, kind: e.kind, is_symlink: e.is_symlink, broken: e.broken,
    }));

    (rows, Disclosure { shown, hidden_clutter, hidden_type, capped_out, unreadable, total_seen })
}

/// Re-derive `entries`/`disclosure` from the CACHED listing. The keystroke path — NO
/// filesystem access.
///
/// Takes the two EDITOR-owned options only. The filter text and the destination flag are
/// derived from `fb.mode` here, because a caller that passed them could pass the wrong ones:
/// every path that rebuilds entries (initial listing, descend, field edit, filter-toggle
/// change) would otherwise have to remember, and `apply_listing_done` did not.
pub(crate) fn rederive(fb: &mut FileBrowser, show_clutter: bool, types: FileTypeFilter) {
    // Recents rows are SYNTHESIZED (see `recents.rs`) into `fb.listing` at open time — an
    // IMMUTABLE cache, exactly like the directory-backed modes' `fb.listing` — and every
    // keystroke re-derives `fb.entries` FRESH from it. There is no directory, no "..", and no
    // type/clutter policy to apply, so this only fuzzy-ranks the cached rows; but the source
    // must be `fb.listing`, never `fb.entries` itself — filtering `entries` in place would
    // make the narrowed list its own source on the next keystroke, a one-way ratchet that
    // never widens back out as the writer backspaces (the Task 23 defect this fixed).
    if matches!(fb.mode, crate::file_browser::BrowseMode::Recents) {
        let filtered = crate::palette::fuzzy_filter(&fb.listing, &fb.query, |e| e.name.as_str());
        fb.entries = filtered.into_iter().map(|e| crate::file_browser::FileEntry {
            name: e.name, kind: e.kind, is_symlink: e.is_symlink, broken: e.broken,
        }).collect();
        if fb.selected >= fb.entries.len() {
            fb.selected = fb.entries.len().saturating_sub(1);
        }
        fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
        return;
    }
    let opts = FilterOpts {
        show_clutter,
        types,
        // Destination mode also shows output-format siblings so a writer sees what they
        // might clobber (spec §7.4).
        destination: fb.mode.is_destination(),
    };
    // DUAL DUTY: the field IS the filter in destination mode; the query is in the others.
    // `filter_text` is the single place that mapping lives.
    let text = fb.mode.filter_text(&fb.query).to_string();
    let at_root = fb.dir.parent().is_none();
    let (rows, d) = filter_and_rank(
        &fb.listing, at_root, &text, opts, fb.total_seen, fb.unreadable);
    fb.entries = rows;
    fb.disclosure = d;
    if fb.selected >= fb.entries.len() {
        fb.selected = fb.entries.len().saturating_sub(1);
    }
    fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileTypeFilter;
    use crate::fsx::{DirEntryInfo, EntryKind};

    fn e(name: &str, kind: EntryKind) -> DirEntryInfo {
        DirEntryInfo { name: name.into(), raw_name: name.into(), kind,
                       is_symlink: false, broken: false }
    }
    fn broken(name: &str) -> DirEntryInfo {
        DirEntryInfo { name: name.into(), raw_name: name.into(),
                       kind: EntryKind::Unknown, is_symlink: true, broken: true }
    }
    fn opts(show_clutter: bool, types: FileTypeFilter, destination: bool) -> FilterOpts {
        FilterOpts { show_clutter, types, destination }
    }

    #[test]
    fn disclosure_accounts_for_everything_withheld() {
        // §7.4's law: shown + withheld must account for what is really there. This is the
        // arithmetic, asserted directly rather than by matching footer strings.
        let listing = vec![
            e("chapter.md", EntryKind::File),
            e("notes.txt", EntryKind::File),
            e("photo.png", EntryKind::File),      // withheld by type
            e(".hidden", EntryKind::File),        // withheld by clutter
            e(".git", EntryKind::Dir),            // withheld by clutter
            e("drafts", EntryKind::Dir),
        ];
        let (rows, d) = filter_and_rank(&listing, false, "", opts(false, FileTypeFilter::Documents, false), 6, 0);
        assert_eq!(d.hidden_clutter, 2, ".hidden and .git");
        assert_eq!(d.hidden_type, 1, "photo.png");
        assert_eq!(d.shown + d.hidden_clutter + d.hidden_type + d.capped_out, d.total_seen,
            "the disclosure must account for every entry");
        // ".." is a synthetic row, not a listing entry — it must not inflate `shown`.
        assert_eq!(rows.first().map(|r| r.name.as_str()), Some(".."), "parent row first");
        assert_eq!(d.shown, 3, "chapter.md, notes.txt, drafts");
    }

    #[test]
    fn cap_and_unreadable_are_separate_disclosures() {
        // Two DIFFERENT facts: "showing N of M" is normal; "k could not be read" means
        // something is wrong with the filesystem. A single conflated counter is what made
        // the cap/no-silent-drop conflict invisible.
        let listing: Vec<DirEntryInfo> =
            (0..4).map(|i| e(&format!("f{i}.md"), EntryKind::File)).collect();
        let (_rows, d) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 10, 3);
        assert_eq!(d.unreadable, 3, "carried through, NOT folded into the cap number");
        assert_eq!(d.capped_out, 10 - 4 - 3, "capped_out = total_seen - retained - unreadable");
        assert_eq!(d.shown + d.hidden_clutter + d.hidden_type + d.capped_out + d.unreadable,
            d.total_seen, "the full invariant, with unreadable as its own term");
    }

    #[test]
    fn directories_and_broken_links_are_never_withheld() {
        // A filter that hides the path to your file is a filter that lies — and hiding a
        // broken link leaves the writer unable to see why their file appears missing.
        let listing = vec![
            e("archive", EntryKind::Dir),        // not a "document" by extension
            broken("dangling.md"),
            e("photo.png", EntryKind::File),
        ];
        let (rows, _d) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 3, 0);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"archive"), "directories survive the type filter: {names:?}");
        assert!(names.contains(&"dangling.md"), "broken links are never hidden: {names:?}");
        assert!(!names.contains(&"photo.png"), "an ordinary non-document IS withheld");
    }

    #[test]
    fn documents_filter_is_mode_aware() {
        let listing = vec![e("book.docx", EntryKind::File), e("book.md", EntryKind::File)];
        let (sel, _) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 2, 0);
        assert!(!sel.iter().any(|r| r.name == "book.docx"),
            "select mode lists what file::open can actually open — .docx is refused as binary");
        let (dst, _) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, true), 2, 0);
        assert!(dst.iter().any(|r| r.name == "book.docx"),
            "destination mode shows output siblings so a writer sees what they might clobber");
    }

    #[test]
    fn clutter_is_dotfiles_and_vcs_dirs_only_no_gitignore() {
        assert!(is_clutter(".hidden"));
        assert!(is_clutter(".git"));
        assert!(is_clutter(".jj"));
        assert!(!is_clutter("notes.md"));
        assert!(!is_clutter("Makefile"), "no gitignore semantics — decision 2");
    }

    /// Feed one printable character through the real intercept.
    fn press_char(e: &mut crate::editor::Editor,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>, c: char)
    {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: tx, fs };
        let ev = Event::Key(KeyEvent {
            code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let _ = crate::file_browser_intercept::intercept(crate::app::Msg::Input(ev), e, &ctx);
    }

    #[test]
    fn typing_in_destination_mode_narrows_the_listing_to_matching_files() {
        // Spec §7.4's overwrite awareness: the field is simultaneously the filename-to-be
        // AND a live filter, so typing `chap` reveals existing chapter files a writer might
        // clobber. This failed silently when `apply_listing_done` hardcoded
        // `destination: false` and re-derived from the (empty) `query`.
        //
        // FAIL-VERIFY: make `rederive` filter on `fb.query` instead of
        // `fb.mode.filter_text(...)`, watch this fail, then revert.
        let d = std::env::temp_dir().join(format!("wc-destfilter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        for n in ["chapter-one.md", "chapter-two.md", "notes.md", "outline.md"] {
            std::fs::write(d.join(n), b"x").expect("seed");
        }
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Construct the destination browser from THIS task's own types and start the listing
        // directly — `Editor::open_destination_picker` belongs to Task 21, and using it here
        // would be a forward reference that blocks this task under TDD.
        let mut fb = crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: String::new(), field_cursor: 0,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        };
        crate::file_browser::start_listing(&mut fb, d.clone(), &fs, &tx);
        e.file_browser = Some(fb);
        // The listing arrives asynchronously — pump it, exactly as the run loop would.
        crate::test_support::pump_listing(&mut e, &rx);
        assert!(e.file_browser.as_ref().expect("open").entries.len() >= 4,
            "precondition: all four files listed before typing");

        // Type through the REAL intercept, one keystroke at a time.
        for c in ['c', 'h', 'a', 'p'] { press_char(&mut e, &fs, &tx, c); }

        let names: Vec<String> = e.file_browser.as_ref().expect("still open")
            .entries.iter().map(|r| r.name.clone()).collect();
        assert!(names.iter().any(|n| n == "chapter-one.md"),
            "existing chapter files must be REVEALED as the writer types: {names:?}");
        assert!(names.iter().any(|n| n == "chapter-two.md"), "{names:?}");
        assert!(!names.iter().any(|n| n == "notes.md"),
            "non-matching files must be filtered out: {names:?}");
        // And the field still holds what was typed — it is dual-duty, not consumed.
        match &e.file_browser.as_ref().expect("open").mode {
            crate::file_browser::BrowseMode::Destination { field, .. } =>
                assert_eq!(field, "chap", "the field is the filename-to-be as well as the filter"),
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn destination_mode_reveals_output_siblings_the_select_mode_hides() {
        // The other half of the Task 18 fix. `typing_in_destination_mode_narrows_the_listing…`
        // above covers the FILTER TEXT half of `rederive`'s internal derivation but seeds only
        // `.md` files, which satisfy `is_document` regardless of `destination`'s value — it
        // never exercises the flag. `documents_filter_is_mode_aware` exercises the flag, but
        // by calling `filter_and_rank` directly with a hand-built `FilterOpts`, bypassing
        // `fb.mode` -> `rederive` -> `apply_listing_done` entirely. A reviewer hardcoded
        // `destination: false` inside `rederive` and all 33 `file_browser*` tests still passed.
        //
        // This drives the REAL wiring in both modes: destination mode via `start_listing` +
        // `apply_listing_done` (constructed by hand — `Editor::open_destination_picker` is a
        // later task, same rationale as the sibling test above), select mode via the real
        // `Editor::open_file_browser` entry point.
        //
        // FAIL-VERIFY: hardcode `destination: false` inside `rederive`, watch this fail, revert.
        let d = std::env::temp_dir().join(format!("wc-destsibling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("chapter.md"), b"x").expect("seed md");
        std::fs::write(d.join("chapter.docx"), b"x").expect("seed docx");
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);

        // Destination mode.
        let mut e_dst = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let mut fb = crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: String::new(), field_cursor: 0,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        };
        crate::file_browser::start_listing(&mut fb, d.clone(), &fs, &tx);
        e_dst.file_browser = Some(fb);
        crate::test_support::pump_listing(&mut e_dst, &rx);
        let dst_names: Vec<String> = e_dst.file_browser.as_ref().expect("open")
            .entries.iter().map(|r| r.name.clone()).collect();
        assert!(dst_names.iter().any(|n| n == "chapter.docx"),
            "destination mode must reveal output-format siblings: {dst_names:?}");

        // Select mode, via the real `Editor::open_file_browser` -> `apply_listing_done` ->
        // `rederive` path.
        let mut e_sel = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let _rx2 = crate::test_support::open_and_pump(&mut e_sel, d.clone());
        let sel_names: Vec<String> = e_sel.file_browser.as_ref().expect("open")
            .entries.iter().map(|r| r.name.clone()).collect();
        assert!(!sel_names.iter().any(|n| n == "chapter.docx"),
            "select mode must hide output-format siblings: {sel_names:?}");
        assert!(sel_names.iter().any(|n| n == "chapter.md"),
            "sanity: the .md file is still shown in select mode: {sel_names:?}");

        let _ = std::fs::remove_dir_all(&d);
    }
}
