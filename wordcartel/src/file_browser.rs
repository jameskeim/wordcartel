//! File-browser overlay: lists the entries of a directory, filters by query,
//! navigates into directories (and `..`), and opens a file on selection.
//! Mirrors the theme picker (theme_picker.rs) / command palette.

use std::path::PathBuf;
use crate::app::Msg;
use crossterm::event::Event;

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

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub dir: PathBuf,
    pub query: String,
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

/// The options this browser filters by, derived from the editor's persisted
/// clutter/type-filter settings. `destination` is always `false` here — every caller in
/// this task is a select (Open) path; a future destination-mode caller (Save-As, Export)
/// passes `true` explicitly instead of this helper.
fn current_filter_opts(editor: &crate::editor::Editor) -> crate::file_browser_listing::FilterOpts {
    crate::file_browser_listing::FilterOpts {
        show_clutter: editor.files_show_clutter,
        types: editor.files_type_filter,
        destination: false,
    }
}

/// `ls -F`-style label, composed with the directory slash.
///
/// TEXT suffixes, not colours, so they survive terminal-plain / no-color mode — the
/// project's standing constraint on every affordance.
pub(crate) fn entry_label(e: &FileEntry) -> String {
    let slash = if matches!(e.kind, crate::fsx::EntryKind::Dir) { "/" } else { "" };
    let link = if e.is_symlink { "@" } else { "" };
    let broken = if e.broken { " (broken)" } else { "" };
    format!("{}{slash}{link}{broken}", e.name)
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

/// File browser overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
/// fall through to normal handling while the browser stays open (mirrors theme_picker).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
    if editor.file_browser.is_none() { return crate::app::Handled::Pass(msg); }
    // Drop an async clipboard-paste result that arrives while the browser is open —
    // it must not land in the document behind the overlay (Codex I6, mirror palette).
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    if let Msg::Input(Event::Paste(text)) = &msg {
        let ah = editor.active().view.area.1;
        let opts = current_filter_opts(editor);
        if let Some(fb) = editor.file_browser.as_mut() {
            fb.query.push_str(text);
            crate::file_browser_listing::rederive(fb, opts);
            crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            match k.code {
                KeyCode::Esc => { editor.file_browser = None; }
                KeyCode::Enter => { file_browser_enter(editor, ctx.fs, ctx.msg_tx); }
                c if crate::list_window::list_nav_key(c).is_some() => {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
                            ah, fb.entries.len(), &mut fb.selected, &mut fb.scroll_top);
                    }
                }
                KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    let opts = current_filter_opts(editor);
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.query.pop();
                        crate::file_browser_listing::rederive(fb, opts);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    let opts = current_filter_opts(editor);
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.query.push(c);
                        crate::file_browser_listing::rederive(fb, opts);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    // Non-key msg falls through to normal handling while the browser stays open.
    crate::app::Handled::Pass(msg)
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
    let opts = crate::file_browser_listing::FilterOpts {
        show_clutter: editor.files_show_clutter,
        types: editor.files_type_filter,
        destination: false,
    };
    if let Some(fb) = editor.file_browser.as_mut() {
        crate::file_browser_listing::rederive(fb, opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fb(dir: PathBuf) -> FileBrowser {
        FileBrowser {
            dir, query: String::new(), listing: vec![], total_seen: 0, unreadable: 0,
            entries: vec![], disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        }
    }

    fn default_opts() -> crate::file_browser_listing::FilterOpts {
        crate::file_browser_listing::FilterOpts {
            show_clutter: false, types: crate::config::FileTypeFilter::Documents, destination: false,
        }
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
        assert_eq!(entry_label(&fe("dangling.md", Unknown, true, true)), "dangling.md@ (broken)");
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
    fn seed_listing(fs: &dyn crate::fsx::Fs, fb: &mut FileBrowser, opts: crate::file_browser_listing::FilterOpts) {
        match fs.list_dir(&fb.dir, Some(crate::limits::MAX_DIR_ENTRIES)) {
            Ok(l) => { fb.listing = l.entries; fb.total_seen = l.total_seen; fb.unreadable = l.unreadable; }
            Err(_) => { fb.listing = Vec::new(); fb.total_seen = 0; fb.unreadable = 0; }
        }
        crate::file_browser_listing::rederive(fb, opts);
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
        crate::file_browser_listing::rederive(&mut fb, default_opts());
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
