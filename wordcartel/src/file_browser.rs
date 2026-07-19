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

/// Execute the selected file-browser entry — the shared Enter path for the keyboard
/// Enter arm and the mouse click-to-commit arm. Descends into a directory (incl. ".."),
/// guarding against unreadable targets, or opens a file through the dirty-guard path.
pub(crate) fn file_browser_enter(fs: &dyn crate::fsx::Fs, editor: &mut crate::editor::Editor) {
    let chosen = editor.file_browser.as_ref().and_then(|fb| {
        fb.entries.get(fb.selected).map(|e| (e.name.clone(), matches!(e.kind, crate::fsx::EntryKind::Dir)))
    });
    if let Some((name, is_dir)) = chosen {
        if is_dir {
            let target = editor.file_browser.as_ref().map(|fb| {
                if name == ".." {
                    fb.dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| fb.dir.clone())
                } else {
                    fb.dir.join(&name)
                }
            });
            if let Some(target) = target {
                // §3: check readability BEFORE committing fb.dir. Routed through the seam
                // (Some(0): this is a readability probe, not a listing — retention isn't
                // needed, only whether the directory can be opened at all).
                if fs.list_dir(&target, Some(0)).is_ok() {
                    let opts = current_filter_opts(editor);
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.dir = target;
                        fb.query.clear();
                        fb.selected = 0;
                        fb.scroll_top = 0; // A6: reset with selected to avoid out-of-order slice
                        crate::file_browser_listing::refetch(fs, fb, opts);
                    }
                } else {
                    editor.set_status_full(crate::status::StatusKind::Error, format!("cannot read directory: {}", target.display()),
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                    // stay in prior dir — do NOT mutate fb.dir
                }
            }
        } else {
            let path = editor.file_browser.as_ref().unwrap().dir.join(&name);
            editor.file_browser = None;
            crate::workspace::open_as_new_buffer(editor, &path);
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
                KeyCode::Enter => { file_browser_enter(&**ctx.fs, editor); }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fb(dir: PathBuf) -> FileBrowser {
        FileBrowser {
            dir, query: String::new(), listing: vec![], total_seen: 0, unreadable: 0,
            entries: vec![], disclosure: Default::default(), selected: 0, scroll_top: 0,
        }
    }

    fn default_opts() -> crate::file_browser_listing::FilterOpts {
        crate::file_browser_listing::FilterOpts {
            show_clutter: false, types: crate::config::FileTypeFilter::Documents, destination: false,
        }
    }

    #[test]
    fn refetch_dirs_first_with_dotdot_and_query_filter() {
        let dir = std::env::temp_dir().join(format!("wc-fb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("alpha.md"), "x").unwrap();
        std::fs::write(dir.join("beta.txt"), "x").unwrap();
        let mut fb = empty_fb(dir.clone());
        crate::file_browser_listing::refetch(&crate::fsx::RealFs, &mut fb, default_opts());
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
        e.open_file_browser(&crate::fsx::RealFs, parent.clone());
        // open_file_browser already fetched+derived entries; "secret" is in the list.
        // Select the "secret" entry (skip ".." which is index 0).
        if let Some(fb) = e.file_browser.as_mut() {
            let idx = fb.entries.iter().position(|en| en.name == "secret").expect("secret dir in entries");
            fb.selected = idx;
        }

        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
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
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &km, &ex, &clk, &tx, &crate::test_support::test_fs());

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
        e.open_file_browser(&crate::fsx::RealFs, std::env::temp_dir());
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
        crate::file_browser_listing::refetch(&crate::fsx::RealFs, &mut fb, default_opts());

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
        crate::file_browser_listing::refetch(&*counting, &mut fb, default_opts());
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
}
