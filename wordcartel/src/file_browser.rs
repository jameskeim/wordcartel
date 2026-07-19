//! File-browser overlay: lists the entries of a directory, filters by query,
//! navigates into directories (and `..`), and opens a file on selection.
//! Mirrors the theme picker (theme_picker.rs) / command palette.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    /// RESOLVED classification from the seam. `File`/`Dir` can BOTH be false — a fifo is
    /// `Other`, an unclassifiable entry is `Unknown`. Consumers match exhaustively on this
    /// rather than testing "is it a directory", so neither falls into a file branch.
    pub kind: crate::fsx::EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

/// What a destination is FOR. The commit path dispatches on this, so adding a future
/// destination consumer is one variant plus one arm the compiler demands — a registration
/// seam, not a growing hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationPurpose {
    SaveAs,
    WriteBlock,
    Export { ext: String },
}

/// Select mode chooses an existing entry; destination mode navigates AND names.
///
/// Not a second `OverlayId`: two overlays would duplicate the intercept, painter, mouse fn,
/// and geometry, and would have to be kept in lockstep by hand — the hand-parallel pathology
/// H21 removed.
#[derive(Debug, Clone)]
pub enum BrowseMode {
    Select,
    Destination {
        purpose: DestinationPurpose,
        /// DUAL-DUTY: simultaneously the filename-to-be and a live filter over the listing,
        /// so typing `chap` narrows to existing chapter files — overwrite awareness for free.
        field: String,
        /// Byte offset into `field`.
        field_cursor: usize,
    },
}

impl BrowseMode {
    pub fn is_destination(&self) -> bool { matches!(self, BrowseMode::Destination { .. }) }
    /// The text the listing filter should use: the query in select mode, the field in
    /// destination mode. One accessor so the two modes cannot drift apart.
    pub fn filter_text<'a>(&'a self, query: &'a str) -> &'a str {
        match self { BrowseMode::Select => query,
                     BrowseMode::Destination { field, .. } => field }
    }
}

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub dir: PathBuf,
    pub query: String,
    /// Select vs destination — see [`BrowseMode`]. Determines both what Enter/click/Tab do
    /// and (via `filter_text`) what the listing filter reads.
    pub mode: BrowseMode,
    /// UNFILTERED contents of `dir`, fetched ONCE per directory. The keystroke path filters
    /// this, never the filesystem — `rebuild_entries` used to re-run `read_dir` on every
    /// character typed.
    pub listing: Vec<crate::fsx::DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
    /// Derived view: filtered, ranked, with the synthetic "..".
    pub entries: Vec<FileEntry>,
    /// `pub(crate)`, not `pub`: `Disclosure` is itself `pub(crate)` (an internal cache
    /// detail), so a `pub` field here would leak a private type through a public struct
    /// (`private_interfaces`, a build-clean GATE). Every consumer is in-crate.
    pub(crate) disclosure: crate::file_browser_listing::Disclosure,
    pub selected: usize,
    /// First visible row index — drives the windowed painter (A6).
    pub scroll_top: usize,
    /// The epoch this browser awaits. Compared against `Msg::ListingDone::epoch`.
    pub awaiting_epoch: u64,
    /// The directory a listing is in flight FOR. `fb.dir` does not move until that listing
    /// succeeds, so the picker shows where the writer actually is until they have actually
    /// arrived — and an unreadable directory never moves them at all.
    pub pending_dir: Option<PathBuf>,
}

/// Directory-listing label in the spirit of `ls -F` — a trailing mark declares what an
/// entry is — but honest about the granularity we actually retain. `ls -F` distinguishes a
/// fifo (`|`) from a socket (`=`) from a door (`>`); `EntryKind::Other` deliberately collapses
/// all of those (plus block/char devices) into one fact, so this uses one generic mark (`%`)
/// for the whole class rather than claim a specific kind we don't know. `?` marks `Unknown` —
/// the `file_type()` probe itself failed, or (via `broken`) a symlink's target could not be
/// resolved: a name with no type. `/` and `@` keep their standard `ls -F` meanings (directory,
/// symlink).
///
/// TEXT suffixes, not colours, so they survive terminal-plain / no-color mode — the
/// project's standing constraint on every affordance.
pub(crate) fn entry_label(e: &FileEntry) -> String {
    let mark = match e.kind {
        crate::fsx::EntryKind::Dir => "/",
        crate::fsx::EntryKind::File => "",
        crate::fsx::EntryKind::Other => "%",
        crate::fsx::EntryKind::Unknown => "?",
    };
    let link = if e.is_symlink { "@" } else { "" };
    let broken = if e.broken { " (broken)" } else { "" };
    format!("{}{mark}{link}{broken}", e.name)
}

/// What Enter does with a selected entry.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnterOutcome {
    Descend(std::path::PathBuf),
    Open(std::path::PathBuf),
    /// Shown, marked, and refused — with the reason, which differs between an unopenable
    /// special file and an unresolvable entry.
    Refuse(String),
}

/// What Enter does with an entry. An exhaustive match on `kind`, so `Other` and `Unknown`
/// cannot fall into a branch meant for files.
pub(crate) fn classify_enter(e: &FileEntry, dir: &std::path::Path) -> EnterOutcome {
    if e.name == ".." {
        // The LOGICAL parent — `fb.dir` is deliberately not canonicalized, so this returns
        // where the writer actually came from rather than a symlink target's real parent.
        return EnterOutcome::Descend(
            dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dir.to_path_buf()));
    }
    match e.kind {
        crate::fsx::EntryKind::Dir => EnterOutcome::Descend(dir.join(&e.name)),
        crate::fsx::EntryKind::File => EnterOutcome::Open(dir.join(&e.name)),
        // A fifo/socket/device is CLASSIFIED — we know what it is, and we know
        // `file::open` on it would block. Shown, marked, refused.
        crate::fsx::EntryKind::Other => EnterOutcome::Refuse(
            format!("{} cannot be opened — not a regular file", e.name)),
        // Unclassifiable, including every broken symlink. "cannot be resolved", never
        // "target is gone": `broken` also covers permission denial and resolution loops.
        crate::fsx::EntryKind::Unknown => EnterOutcome::Refuse(if e.broken {
            format!("{} — symlink cannot be resolved", e.name)
        } else {
            format!("{} — type could not be determined", e.name)
        }),
    }
}

/// The destination-mode footer: `→ /abs/path/after-policy.md`, plus an inline note when the
/// target already exists.
///
/// Shows the POST-POLICY name so the `.md` that policy appends is visible before commit, and
/// the RESOLVED path when a symlink changed it — resolution should be visible up front, not
/// discovered in a confirm dialog.
pub(crate) fn footer_target(fs: &dyn crate::fsx::Fs, fb: &FileBrowser) -> Option<String> {
    let BrowseMode::Destination { field, purpose, .. } = &fb.mode else { return None };
    if field.trim().is_empty() { return None; }
    let typed = crate::file_browser_commit::resolve_field(&fb.dir, field);
    // An export destination's extension is fixed by the format — policy does not apply.
    let after_policy = if matches!(purpose, DestinationPurpose::Export { .. }) {
        typed
    } else {
        match crate::file_browser_commit::apply_extension_policy(&typed) {
            crate::file_browser_commit::ExtVerdict::Defaulted(p) => p,
            crate::file_browser_commit::ExtVerdict::Honoured(p) => p,
            crate::file_browser_commit::ExtVerdict::Redirect { path, ext } => {
                return Some(format!("\u{2192} {} \u{2014} {ext} is an export format",
                    path.display()));
            }
            // `ExtVerdict` grew this fourth arm in Task 19's review, after this footer's
            // brief was written — flagged per the dispatch note rather than silently
            // invented. A trailing separator names a directory the writer is asking to
            // create/enter, not a file: same "no alternate flow, just fix the field"
            // framing as `apply_extension_policy`'s own doc comment on `Refused`.
            crate::file_browser_commit::ExtVerdict::Refused(path) => {
                return Some(format!("\u{2192} {} \u{2014} names a directory, not a file",
                    path.display()));
            }
        }
    };
    let shown = match crate::fsx::resolve_write_destination(fs, &after_policy) {
        Ok(r) => r,
        Err(crate::fsx::DestError::BrokenSymlink) => {
            return Some(format!("\u{2192} {} \u{2014} symlink cannot be resolved",
                after_policy.display()));
        }
    };
    let note = if crate::fsx::exists_via(fs, &shown) { " (exists \u{2014} will confirm)" } else { "" };
    Some(format!("\u{2192} {}{note}", shown.display()))
}

/// Execute the selected file-browser entry — the shared Enter path for the keyboard
/// Enter arm and the mouse click-to-commit arm. Descends into a directory (incl. "..")
/// by spawning an off-thread listing (Task 13) — `fb.dir`/`query`/`selected`/`scroll_top`
/// do NOT move here; they move together in `apply_listing_done`'s success arm, so an
/// unreadable target costs the writer nothing. Opens a file through the dirty-guard path.
/// Refuses `Other`/`Unknown` entries with a status message — shown, marked, refused, never
/// silently skipped — and leaves the picker open so the writer can pick something else.
pub(crate) fn file_browser_enter(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let chosen = editor.file_browser.as_ref().and_then(|fb| {
        fb.entries.get(fb.selected).cloned().map(|e| (e, fb.dir.clone()))
    });
    let Some((entry, dir)) = chosen else { return };
    match classify_enter(&entry, &dir) {
        EnterOutcome::Descend(target) => {
            if let Some(fb) = editor.file_browser.as_mut() {
                // Does NOT touch fb.dir / query / selected / scroll_top. All four move
                // together in `apply_listing_done`'s success arm, so an unreadable target
                // leaves the writer exactly where they were — with their query intact.
                start_listing(fb, target, fs, msg_tx);
            }
        }
        EnterOutcome::Open(path) => {
            editor.file_browser = None;
            crate::workspace::open_as_new_buffer(editor, &path);
        }
        EnterOutcome::Refuse(msg) => {
            // Shown and marked, but not actioned — and the picker STAYS OPEN so the writer
            // can pick something else.
            editor.set_status_full(crate::status::StatusKind::Warning, msg,
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
}

/// The shared click-commit path for `mouse::mouse_file_browser`'s `Down(Left)` arm.
///
/// SELECT mode selects and commits, as it always has — the caller invokes
/// `file_browser_enter` separately (this fn's `Select` arm is a deliberate no-op). DESTINATION
/// mode copies the highlighted file's name into the field and stops: a single click must never
/// reach a write. The stakes are asymmetric — a mis-click in select mode opens the wrong file
/// (close the buffer), a mis-click in destination mode would land on the overwrite path for an
/// existing file. The inconsistency between the two modes IS the safety property; do not
/// "unify" them.
pub(crate) fn click_commit_or_copy(editor: &mut crate::editor::Editor) {
    let Some(fb) = editor.file_browser.as_mut() else { return };
    let Some(entry) = fb.entries.get(fb.selected).cloned() else { return };
    match &mut fb.mode {
        BrowseMode::Select => { /* caller invokes file_browser_enter — unchanged */ }
        BrowseMode::Destination { field, field_cursor, .. } => {
            if matches!(entry.kind, crate::fsx::EntryKind::File) {
                crate::file_browser_commit::copy_name_into_field(field, field_cursor, &entry.name);
            }
            // Dir/Other/Unknown: the click has already moved the highlight; nothing else.
        }
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic listing epoch, PROCESS-GLOBAL by design.
///
/// It is deliberately not a `FileBrowser` field: closing the picker would drop a per-browser
/// counter, and a freshly-opened picker would start from the same value — so a stale result
/// from the previous picker's in-flight listing could carry a matching epoch and be accepted
/// (an ABA bug). A global counter never reissues a value, so the match is unforgeable.
pub(crate) static LISTING_EPOCH: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_epoch() -> u64 {
    LISTING_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// Spawn a listing for `target` on its own thread.
///
/// `fb.dir` is DELIBERATELY not moved here — see `apply_listing_done`. `pending_dir` and
/// `awaiting_epoch` are stamped together, so "a listing is in flight for X" is one fact in
/// one place and cannot disagree with itself.
///
/// The overlay stays fully closable while this is in flight: closing means the result is
/// discarded on arrival, and the detached thread exits on its own. A stuck mount strands one
/// short-lived thread, never the UI.
pub(crate) fn start_listing(
    fb: &mut FileBrowser,
    target: std::path::PathBuf,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let epoch = next_epoch();
    fb.awaiting_epoch = epoch;
    fb.pending_dir = Some(target.clone());
    let fs = std::sync::Arc::clone(fs);
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        let result = fs.list_dir(&target, Some(crate::limits::MAX_DIR_ENTRIES));
        let _ = tx.send(crate::app::Msg::ListingDone { epoch, dir: target, result });
    });
}

/// Merge a listing result. Discards when there is no active picker, and when the epoch is
/// not the one the active picker awaits. BOTH halves are required.
///
/// On SUCCESS the pending directory and its entries are committed TOGETHER, so the picker
/// never shows a directory it has not actually read. On ERROR `fb.dir` is left untouched:
/// an unreadable directory does not move the writer, it just tells them.
pub(crate) fn apply_listing_done(
    editor: &mut crate::editor::Editor,
    epoch: u64,
    dir: std::path::PathBuf,
    result: std::io::Result<crate::fsx::DirListing>,
) {
    let Some(fb) = editor.file_browser.as_mut() else { return }; // no picker → inert
    if fb.awaiting_epoch != epoch { return; }                    // stale → inert
    debug_assert_eq!(fb.pending_dir.as_deref(), Some(dir.as_path()),
        "the merge must target the directory it listed");
    match result {
        Ok(l) => {
            // Commit the directory move and its contents in one step.
            let moved = fb.pending_dir.take().is_some_and(|p| {
                let changed = p != fb.dir;
                fb.dir = p;
                changed
            });
            fb.listing = l.entries;
            fb.total_seen = l.total_seen;
            fb.unreadable = l.unreadable;
            if moved {
                // Descend resets the view — but only now that we have actually arrived, so a
                // failed descend does not cost the writer the query they had typed.
                fb.query.clear();
                fb.selected = 0;
                fb.scroll_top = 0; // A6: reset with selected to avoid an out-of-order slice
            }
        }
        Err(e) => {
            // fb.dir is NOT touched. The writer stays where they were.
            fb.pending_dir = None;
            editor.set_status_full(crate::status::StatusKind::Error,
                format!("cannot read directory: {} ({e})", dir.display()),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    }
    // The destination flag and filter text are both derived from `fb.mode` INSIDE
    // `rederive` — this site used to hardcode `destination: false`, which silently defeated
    // §7.4's overwrite-awareness filter on every initial open and every descend while in
    // destination mode (the field never gets a chance to filter until the first keystroke).
    let (show_clutter, types) = (editor.files_show_clutter, editor.files_type_filter);
    if let Some(fb) = editor.file_browser.as_mut() {
        crate::file_browser_listing::rederive(fb, show_clutter, types);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fb(dir: PathBuf) -> FileBrowser {
        FileBrowser {
            dir, query: String::new(), mode: BrowseMode::Select,
            listing: vec![], total_seen: 0, unreadable: 0,
            entries: vec![], disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        }
    }

    /// The two editor-owned filter options `rederive` now takes directly (Task 18) — no
    /// `FilterOpts` construction at call sites; the destination flag comes from `fb.mode`.
    fn default_opts() -> (bool, crate::config::FileTypeFilter) {
        (false, crate::config::FileTypeFilter::Documents)
    }

    fn fe(name: &str, kind: crate::fsx::EntryKind, is_symlink: bool, broken: bool) -> FileEntry {
        FileEntry { name: name.into(), kind, is_symlink, broken }
    }

    #[test]
    fn entry_labels_follow_ls_f_and_survive_no_color() {
        use crate::fsx::EntryKind::*;
        // TEXT suffixes, never colour — the terminal-plain constraint. This also restores
        // the trailing '/' that a symlinked directory used to lose entirely (§4.9).
        assert_eq!(entry_label(&fe("drafts", Dir, false, false)), "drafts/");
        assert_eq!(entry_label(&fe("linked", Dir, true, false)), "linked/@");
        assert_eq!(entry_label(&fe("notes.md", File, false, false)), "notes.md");
        assert_eq!(entry_label(&fe("alias.md", File, true, false)), "alias.md@");
        assert_eq!(entry_label(&fe("dangling.md", Unknown, true, true)), "dangling.md?@ (broken)");
    }

    /// Task 14 gave `Other` and `Unknown` refusals in `classify_enter`, but `entry_label` had
    /// no mark for either — a bare `Other` (fifo/socket/device) rendered byte-identical to a
    /// plain file, so a writer only learned otherwise when Enter refused it. Covers the four
    /// shapes the review found under-tested: bare `Other`, bare `Unknown`, a symlinked `Other`,
    /// and a broken entry — each must stay distinguishable from a plain `File` and from
    /// each other, per the same law that made `EntryKind` an enum instead of `is_file`/`is_dir`.
    #[test]
    fn entry_label_marks_other_and_unknown_distinctly() {
        use crate::fsx::EntryKind::*;

        // A bare Other (no symlink, not broken) must not look like an ordinary file.
        let pipe = entry_label(&fe("pipe", Other, false, false));
        let file = entry_label(&fe("pipe", File, false, false));
        assert_eq!(pipe, "pipe%");
        assert_ne!(pipe, file, "a fifo/socket/device must not render like a regular file");

        // A bare Unknown (file_type() probe failed, not a symlink) is the same gap for the
        // other unclassifiable kind.
        let unknown = entry_label(&fe("mystery", Unknown, false, false));
        assert_eq!(unknown, "mystery?");
        assert_ne!(unknown, file, "an unclassified entry must not render like a regular file");
        assert_ne!(unknown, pipe,
            "Other and Unknown are different facts (§ fsx.rs EntryKind doc) and must render \
             differently, not share a mark");

        // A symlinked Other keeps the special-file mark AND the link mark.
        let linked_pipe = entry_label(&fe("pipe-link", Other, true, false));
        assert_eq!(linked_pipe, "pipe-link%@");
        assert_ne!(linked_pipe, pipe, "the symlink mark must still distinguish the two");

        // A broken entry (always Unknown + symlink, by the `broken` invariant) stays
        // distinguishable from all three above.
        let broken = entry_label(&fe("dangling.md", Unknown, true, true));
        assert_eq!(broken, "dangling.md?@ (broken)");
        assert_ne!(broken, unknown, "the ' (broken)' text must still set it apart");
        assert_ne!(broken, linked_pipe, "a broken Unknown must not read like a symlinked Other");
    }

    #[test]
    fn classify_enter_covers_every_kind_exhaustively() {
        use crate::fsx::EntryKind::*;
        let d = std::path::Path::new("/tmp/wc-classify");
        assert_eq!(classify_enter(&fe("sub", Dir, false, false), d),
            EnterOutcome::Descend(d.join("sub")));
        assert_eq!(classify_enter(&fe("sub", Dir, true, false), d),
            EnterOutcome::Descend(d.join("sub")), "a symlinked dir descends like any dir");
        assert_eq!(classify_enter(&fe("n.md", File, false, false), d),
            EnterOutcome::Open(d.join("n.md")));

        // A fifo must be REFUSED, and for a concrete reason: file::open on a fifo BLOCKS.
        match classify_enter(&fe("pipe", Other, false, false), d) {
            EnterOutcome::Refuse(msg) => assert!(msg.to_lowercase().contains("cannot be opened"),
                "the reason must name the openability problem, got {msg:?}"),
            other => panic!("a fifo must be refused, got {other:?}"),
        }
        // An unresolvable entry is refused with a DIFFERENT reason — the pair of facts the
        // old bool model could not separate.
        match classify_enter(&fe("dangling.md", Unknown, true, true), d) {
            EnterOutcome::Refuse(msg) => assert!(msg.to_lowercase().contains("cannot be resolved"),
                "must say cannot-be-resolved, NOT 'target is gone' — broken also covers \
                 permission and loop failures, got {msg:?}"),
            other => panic!("a broken link must be refused, got {other:?}"),
        }
        // A non-broken Unknown — a bare `file_type()` probe failure, reachable in production
        // at fsx.rs's `Err(_) => (EntryKind::Unknown, false, false)` path, NOT a symlink — must
        // get the OTHER reason. Collapsing this arm to always return the broken-symlink message
        // is a real mutation that left every prior test green; this guards against it.
        match classify_enter(&fe("mystery", Unknown, false, false), d) {
            EnterOutcome::Refuse(msg) => {
                assert!(msg.to_lowercase().contains("could not be determined"),
                    "a non-broken Unknown must say type-could-not-be-determined, got {msg:?}");
                assert!(!msg.to_lowercase().contains("cannot be resolved"),
                    "a non-broken Unknown must NOT reuse the broken-symlink reason, got {msg:?}");
            }
            other => panic!("a non-broken Unknown must still be refused, got {other:?}"),
        }
    }

    #[test]
    fn dotdot_descends_to_the_logical_parent() {
        let d = std::path::Path::new("/tmp/wc-classify/sub");
        assert_eq!(classify_enter(&fe("..", crate::fsx::EntryKind::Dir, false, false), d),
            EnterOutcome::Descend(std::path::PathBuf::from("/tmp/wc-classify")),
            "'..' walks the LOGICAL parent — fb.dir is deliberately not canonicalized, so a \
             writer who descended through a symlink leaves by the path they came in on");
    }

    /// Test-only stand-in for the deleted synchronous `refetch`: one `list_dir` call, then
    /// derive. Production now fetches only off-thread (`start_listing` + `apply_listing_done`);
    /// these listing-pipeline tests still want a one-shot synchronous fetch to seed `fb.listing`.
    fn seed_listing(fs: &dyn crate::fsx::Fs, fb: &mut FileBrowser, opts: (bool, crate::config::FileTypeFilter)) {
        match fs.list_dir(&fb.dir, Some(crate::limits::MAX_DIR_ENTRIES)) {
            Ok(l) => { fb.listing = l.entries; fb.total_seen = l.total_seen; fb.unreadable = l.unreadable; }
            Err(_) => { fb.listing = Vec::new(); fb.total_seen = 0; fb.unreadable = 0; }
        }
        crate::file_browser_listing::rederive(fb, opts.0, opts.1);
    }

    #[test]
    fn refetch_dirs_first_with_dotdot_and_query_filter() {
        let dir = std::env::temp_dir().join(format!("wc-fb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("alpha.md"), "x").unwrap();
        std::fs::write(dir.join("beta.txt"), "x").unwrap();
        let mut fb = empty_fb(dir.clone());
        seed_listing(&crate::fsx::RealFs, &mut fb, default_opts());
        assert_eq!(fb.entries[0].name, "..", "parent first");
        let names: Vec<_> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        let sub_i = names.iter().position(|n| *n == "sub").unwrap();
        let alpha_i = names.iter().position(|n| *n == "alpha.md").unwrap();
        assert!(sub_i < alpha_i, "directories sort before files");
        fb.query = "alpha".into();
        let (sc, tf) = default_opts();
        crate::file_browser_listing::rederive(&mut fb, sc, tf);
        assert!(fb.entries.iter().any(|e| e.name == "alpha.md"));
        assert!(!fb.entries.iter().any(|e| e.name == "beta.txt"), "query filters out non-matching names");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Advisory #2 regression: Enter on an unreadable directory must keep fb.dir unchanged
    // and set a status — the browser must NOT descend into an unreadable dir.
    #[test]
    #[cfg(unix)]
    fn enter_on_unreadable_dir_stays_put_and_sets_status() {
        use crate::editor::Editor;
        use crate::app::Msg;
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        use std::os::unix::fs::PermissionsExt;

        let parent = std::env::temp_dir().join(format!("wc-fb-unreadable-{}", std::process::id()));
        let secret = parent.join("secret");
        std::fs::create_dir_all(&secret).unwrap();
        // chmod 000: read_dir will fail
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut e = Editor::new_from_text("x\n", None, (40, 12));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = std::sync::Arc::new(crate::fsx::RealFs);
        e.open_file_browser(&fs, &tx, parent.clone());
        // The listing runs on its own thread now — pump it before reading `entries`.
        crate::test_support::pump_listing(&mut e, &rx);
        // "secret" is in the list. Select it (skip ".." which is index 0).
        if let Some(fb) = e.file_browser.as_mut() {
            let idx = fb.entries.iter().position(|en| en.name == "secret").expect("secret dir in entries");
            fb.selected = idx;
        }

        let reg = Registry::builtins();
        let km = {
            let (t, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
            t
        };
        let ex = InlineExecutor::default();
        struct LocalClock;
        impl wordcartel_core::history::Clock for LocalClock { fn now_ms(&self) -> u64 { 0 } }
        let clk = LocalClock;
        let enter = Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &km, &ex, &clk, &tx, &fs);
        // The descend spawned a listing that fails (chmod 000) — pump its result, exactly
        // as the run loop would deliver it.
        crate::test_support::pump_listing(&mut e, &rx);

        // Dir must NOT have changed — still at parent.
        let fb_dir = e.file_browser.as_ref().map(|fb| fb.dir.clone());
        assert_eq!(fb_dir.as_deref(), Some(parent.as_path()),
            "fb.dir must remain at parent after Enter on unreadable dir, got: {:?}", fb_dir);
        // Status must mention the unreadable directory.
        assert!(e.status_text().contains("cannot read directory"),
            "status must mention 'cannot read directory', got: {:?}", e.status_text());
        // A17 T4: a genuine "cannot read directory" failure must land Sticky/Error —
        // surviving a later Info ack (Q1), not clearing on the next keystroke.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");

        // Restore permissions so cleanup can remove the dir.
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn open_file_browser_enforces_xor() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 12));
        e.open_palette();
        let (tx, _rx) = std::sync::mpsc::channel();
        e.open_file_browser(&crate::test_support::test_fs(), &tx, std::env::temp_dir());
        assert!(e.file_browser.is_some());
        assert!(e.palette.is_none(), "opening file_browser clears the palette (XOR)");
    }

    #[cfg(unix)]
    #[test]
    fn refetch_treats_a_symlinked_directory_as_a_directory() {
        // §4.9 REGRESSION. `DirEntry::file_type()` does not follow symlinks, so a symlink to
        // a directory reported is_dir == false: it sorted with FILES, rendered without the
        // trailing '/', and Enter routed it to file::open, which returned "is a directory".
        // The entry was UNUSABLE, not merely mis-sorted. Routing through the resolving
        // list_dir fixes all three consumers at once.
        let dir = std::env::temp_dir().join(format!("wc-fb-symdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real_sub")).expect("seed dir");
        std::fs::write(dir.join("plain.md"), b"x").expect("seed file");
        std::os::unix::fs::symlink(dir.join("real_sub"), dir.join("link_sub")).expect("symlink");

        let mut fb = empty_fb(dir.clone());
        seed_listing(&crate::fsx::RealFs, &mut fb, default_opts());

        let link = fb.entries.iter().find(|e| e.name == "link_sub").expect("link listed");
        assert!(matches!(link.kind, crate::fsx::EntryKind::Dir),
            "a symlink to a directory MUST classify as a directory");

        let names: Vec<&str> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        let link_i = names.iter().position(|n| *n == "link_sub").expect("present");
        let file_i = names.iter().position(|n| *n == "plain.md").expect("present");
        assert!(link_i < file_i, "and therefore sorts with the directories, before files");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dominant responsiveness defect: a query keystroke performs no directory read.
    /// Counted through the seam, not timed — a timing test would be flaky and would not
    /// fail if someone reintroduced the syscall on a fast disk.
    #[test]
    fn a_query_keystroke_performs_no_directory_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CountingFs { inner: crate::fsx::RealFs, calls: AtomicUsize }
        impl crate::fsx::Fs for CountingFs {
            fn create_excl(&self, p: &std::path::Path, m: u32)
                -> std::io::Result<Box<dyn crate::fsx::WriteSync>> { self.inner.create_excl(p, m) }
            fn existing_mode(&self, p: &std::path::Path) -> Option<u32> { self.inner.existing_mode(p) }
            fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> std::io::Result<()> { self.inner.rename(a, b) }
            fn sync_dir(&self, d: &std::path::Path) -> std::io::Result<()> { self.inner.sync_dir(d) }
            fn remove_file(&self, p: &std::path::Path) -> std::io::Result<()> { self.inner.remove_file(p) }
            fn read_capped(&self, p: &std::path::Path, l: u64) -> std::io::Result<Option<Vec<u8>>> {
                self.inner.read_capped(p, l)
            }
            fn stat(&self, p: &std::path::Path) -> std::io::Result<crate::fsx::FileStat> { self.inner.stat(p) }
            fn list_dir(&self, p: &std::path::Path, cap: Option<usize>)
                -> std::io::Result<crate::fsx::DirListing>
            {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.inner.list_dir(p, cap)
            }
        }

        let dir = std::env::temp_dir().join(format!("wc-fb-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        for n in ["alpha.md", "beta.md", "gamma.md"] {
            std::fs::write(dir.join(n), b"x").expect("seed");
        }
        // Wrapped in an `Arc<CountingFs>` FIRST, then coerced to the trait-object Arc the
        // intercept needs — both point at the SAME counter, so `counting.calls` still
        // reads live after the trait-object clone is handed to the keystroke path.
        let counting = std::sync::Arc::new(CountingFs { inner: crate::fsx::RealFs, calls: AtomicUsize::new(0) });
        let mut fb = empty_fb(dir.clone());
        seed_listing(&*counting, &mut fb, default_opts());
        assert_eq!(counting.calls.load(Ordering::Relaxed), 1, "one fetch on open");

        // Keystrokes go through the REAL intercept — mutating `fb.query` and calling
        // `rederive` by hand would prove nothing about the path a writer's typing takes, and
        // would still pass if the intercept re-fetched on every character.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 12));
        let (tx, _rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let fs_arc: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = counting.clone();
        e.file_browser = Some(fb);
        for c in ['a', 'l', 'p'] { crate::test_support::press_char_fb(&mut e, &fs_arc, &tx, c); }
        assert_eq!(counting.calls.load(Ordering::Relaxed), 1,
            "THREE keystrokes through the intercept performed ZERO additional list_dir calls");
        assert!(e.file_browser.as_ref().unwrap().entries.iter().any(|x| x.name == "alpha.md"),
            "and the filter still works");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_listing_after_close_and_reopen_is_discarded() {
        // FAIL-VERIFY: move the epoch onto FileBrowser, watch this fail, then revert.
        //
        // THE ABA CASE. If the epoch lived on FileBrowser, closing would drop it and the
        // reopened picker would restart at the same value — so the FIRST picker's still
        // in-flight listing would carry a matching epoch and be accepted, painting the wrong
        // directory. A process-global counter never reissues, so the match is unforgeable.
        //
        // Fast listings hide this: the window only opens when a listing OUTLIVES the picker
        // that started it, which is exactly the hung-mount case the thread exists for.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dir_a = std::env::temp_dir().join(format!("wc-aba-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("wc-aba-b-{}", std::process::id()));
        for d in [&dir_a, &dir_b] { let _ = std::fs::remove_dir_all(d); std::fs::create_dir_all(d).expect("dir"); }
        std::fs::write(dir_a.join("from_a.md"), b"x").expect("seed");
        std::fs::write(dir_b.join("from_b.md"), b"x").expect("seed");

        // Picker #1 over dir_a; capture the epoch it is awaiting, then CLOSE it.
        let _rx_a = crate::test_support::open_and_pump(&mut e, dir_a.clone());
        let stale_epoch = e.file_browser.as_ref().expect("open").awaiting_epoch;
        e.file_browser = None;

        // Picker #2 over dir_b.
        let _rx_b = crate::test_support::open_and_pump(&mut e, dir_b.clone());
        let fresh_epoch = e.file_browser.as_ref().expect("reopen").awaiting_epoch;
        assert_ne!(stale_epoch, fresh_epoch, "a global epoch never reissues across close/reopen");

        // Picker #1's listing finally lands.
        let stale = crate::fsx::DirListing {
            entries: vec![crate::fsx::DirEntryInfo {
                name: "from_a.md".into(), raw_name: "from_a.md".into(),
                kind: crate::fsx::EntryKind::File, is_symlink: false, broken: false }],
            total_seen: 1, unreadable: 0,
        };
        apply_listing_done(&mut e, stale_epoch, dir_a.clone(), Ok(stale));

        let names: Vec<String> =
            e.file_browser.as_ref().expect("still open").entries.iter().map(|r| r.name.clone()).collect();
        assert!(!names.iter().any(|n| n == "from_a.md"),
            "the stale listing must be discarded: {names:?}");
        assert_eq!(e.file_browser.as_ref().expect("open").dir, dir_b, "picker #2 is untouched");
        for d in [&dir_a, &dir_b] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    #[cfg(unix)]   // chmod-based unreadability is meaningless off Unix
    #[allow(clippy::print_stderr)] // env-conditional skip notice — mirrors fsx.rs's harness allow
    fn a_failed_descend_leaves_the_writer_exactly_where_they_were() {
        // `chmod 000` does not restrict root or CAP_DAC_OVERRIDE. If the listing SUCCEEDS the
        // premise is void — skip rather than assert an inverted result, because a test that
        // passes for the wrong reason is worse than one that opts out loudly.
        if crate::test_support::nix_privileged() {
            eprintln!("skip: privileged process — chmod 000 does not restrict this test");
            return;
        }
        // The hold-pending guarantee. `fb.dir` does not move on Enter — it moves only when a
        // listing for the target SUCCEEDS. So an unreadable target costs the writer nothing:
        // not their directory, not their query, not their selection.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dir = std::env::temp_dir().join(format!("wc-faildescend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("keep.md"), b"x").expect("seed");

        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = std::sync::Arc::new(crate::fsx::RealFs);
        e.open_file_browser(&fs, &tx, dir.clone());
        crate::test_support::pump_listing(&mut e, &rx);

        // Drive a REAL descend. Hand-stamping `awaiting_epoch`/`pending_dir` and calling
        // `apply_listing_done` would test only the error arm — if `file_browser_enter`'s
        // Descend arm moved `fb.dir`/`query` eagerly (the guarantee being claimed), that
        // version passes.
        //
        // FAIL-VERIFY: make the Descend arm set `fb.dir = target` and clear `fb.query` before
        // spawning, watch this fail on both the dir and the query assertion.
        let bad = dir.join("unreadable");
        std::fs::create_dir_all(&bad).expect("dir");
        std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .expect("chmod 000 so the listing fails");
        e.file_browser.as_mut().expect("open").query.push_str("ke");
        // Select the unreadable directory and press Enter through the real intercept.
        {
            let fb = e.file_browser.as_mut().expect("open");
            fb.entries = vec![crate::file_browser::FileEntry {
                name: "unreadable".into(), kind: crate::fsx::EntryKind::Dir,
                is_symlink: false, broken: false }];
            fb.selected = 0;
        }
        crate::test_support::press_enter_fb(&mut e, &fs, &tx);
        // The listing fails on its thread; pump the result the run loop would deliver.
        crate::test_support::pump_listing(&mut e, &rx);

        std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o755)).ok();

        let fb = e.file_browser.as_ref().expect("picker stays open");
        assert_eq!(fb.dir, dir, "a failed descend does NOT move the writer");
        assert_eq!(fb.query, "ke", "and does not cost them the query they had typed");
        assert!(fb.pending_dir.is_none(), "the pending target is cleared");
        assert!(e.status_text().contains("cannot read directory"), "and they are told");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn footer_shows_the_post_policy_absolute_target() {
        // The .md that policy appends must be visible BEFORE commit, not discovered after.
        let d = std::env::temp_dir().join(format!("wc-footer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let mut fb = FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: BrowseMode::Destination {
                purpose: DestinationPurpose::SaveAs,
                field: "chapter one".into(), field_cursor: 11,
            },
            listing: vec![], total_seen: 0, unreadable: 0, entries: vec![],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        let line = footer_target(&crate::fsx::RealFs, &fb).expect("destination mode has a footer");
        assert!(line.contains(&d.join("chapter one.md").display().to_string()),
            "the footer shows the ABSOLUTE, post-policy target: {line}");
        assert!(!line.contains("will confirm"), "nothing exists there yet");

        // When the target exists, overwrite is telegraphed one step BEFORE the confirm.
        std::fs::write(d.join("taken.md"), b"x").expect("seed");
        if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
            *field = "taken.md".into(); *field_cursor = field.len();
        }
        let line = footer_target(&crate::fsx::RealFs, &fb).expect("footer");
        assert!(line.contains("exists"), "an existing target is disclosed inline: {line}");

        // RESOLUTION must be visible before commit, not discovered in a confirm dialog. If
        // `footer_target` skipped `resolve_write_destination`, it would echo the symlink path
        // and both assertions above would still pass.
        //
        // FAIL-VERIFY: drop the `resolve_write_destination` call from `footer_target`, watch
        // this fail — the footer shows `link.md` instead of the target.
        #[cfg(unix)]
        {
            std::fs::write(d.join("real-target.md"), b"x").expect("seed");
            std::os::unix::fs::symlink(d.join("real-target.md"), d.join("link.md"))
                .expect("symlink");
            if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
                *field = "link.md".into(); *field_cursor = field.len();
            }
            let line = footer_target(&crate::fsx::RealFs, &fb).expect("footer");
            assert!(line.contains("real-target.md"),
                "the footer names the RESOLVED write target, not the link the writer typed: {line}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn footer_is_absent_in_select_mode() {
        let mut fb = FileBrowser {
            dir: std::env::temp_dir(), query: "q".into(), mode: BrowseMode::Select,
            listing: vec![], total_seen: 0, unreadable: 0, entries: vec![],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        assert!(footer_target(&crate::fsx::RealFs, &fb).is_none(), "select mode names no target");
        fb.mode = BrowseMode::Destination {
            purpose: DestinationPurpose::SaveAs, field: String::new(), field_cursor: 0 };
        assert!(footer_target(&crate::fsx::RealFs, &fb).is_none(), "an empty field names none either");
    }

    #[test]
    fn listing_result_with_no_active_picker_is_discarded_without_panic() {
        // Both halves of the discard condition are required: the epoch match, AND
        // "no active picker discards unconditionally". Without the second, the first has
        // nothing to compare against.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        assert!(e.file_browser.is_none(), "precondition: no picker");
        let l = crate::fsx::DirListing { entries: vec![], total_seen: 0, unreadable: 0 };
        apply_listing_done(&mut e, 12345, std::env::temp_dir(), Ok(l));
        assert!(e.file_browser.is_none(), "no picker was resurrected, and no panic");
    }
}
