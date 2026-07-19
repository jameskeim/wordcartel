//! Pure listing pipeline for the file browser: cache -> filter -> rank -> disclosure.
//!
//! No IO except `refetch`, and no `Editor`. Kept separate from `file_browser.rs` on one
//! axis of change: this module answers "which rows exist", not "what a browser is".

use crate::config::FileTypeFilter;
use crate::file_browser::{FileBrowser, FileEntry};
use crate::fsx::{DirEntryInfo, EntryKind, Fs};

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

/// Fetch ONCE for `fb.dir`, then derive. Called on open and descend only.
pub(crate) fn refetch(fs: &dyn Fs, fb: &mut FileBrowser, opts: FilterOpts) {
    match fs.list_dir(&fb.dir, Some(crate::limits::MAX_DIR_ENTRIES)) {
        Ok(l) => {
            fb.listing = l.entries;
            fb.total_seen = l.total_seen;
            fb.unreadable = l.unreadable;
        }
        Err(_) => {
            fb.listing = Vec::new();
            fb.total_seen = 0;
            fb.unreadable = 0;
        }
    }
    rederive(fb, opts);
}

/// Re-derive `entries`/`disclosure` from the CACHED listing. This is the keystroke path and
/// it performs NO filesystem access.
pub(crate) fn rederive(fb: &mut FileBrowser, opts: FilterOpts) {
    let at_root = fb.dir.parent().is_none();
    let (rows, d) = filter_and_rank(
        &fb.listing, at_root, &fb.query, opts, fb.total_seen, fb.unreadable);
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
}
