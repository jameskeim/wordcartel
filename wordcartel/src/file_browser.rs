//! File-browser overlay: lists the entries of a directory, filters by query,
//! navigates into directories (and `..`), and opens a file on selection.
//! Mirrors the theme picker (theme_picker.rs) / command palette.

use std::path::PathBuf;
use crate::app::Msg;
use crossterm::event::Event;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub dir: PathBuf,
    pub query: String,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    /// First visible row index — drives the windowed painter (A6).
    pub scroll_top: usize,
}

/// Rebuild `entries` from `dir`: synthetic ".." first (unless at root), then directories,
/// then files, each alphabetical; substring-filtered (case-insensitive) by `query`.
pub fn rebuild_entries(fb: &mut FileBrowser) {
    let q = fb.query.to_ascii_lowercase();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&fb.dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !q.is_empty() && !name.to_ascii_lowercase().contains(&q) {
                continue;
            }
            if is_dir {
                dirs.push(name)
            } else {
                files.push(name)
            }
        }
    }
    dirs.sort();
    files.sort();
    fb.entries = Vec::new();
    if fb.dir.parent().is_some() {
        fb.entries.push(FileEntry { name: "..".into(), is_dir: true });
    }
    fb.entries.extend(dirs.into_iter().map(|name| FileEntry { name, is_dir: true }));
    fb.entries.extend(files.into_iter().map(|name| FileEntry { name, is_dir: false }));
    if fb.selected >= fb.entries.len() {
        fb.selected = fb.entries.len().saturating_sub(1);
    }
    fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
}

/// Execute the selected file-browser entry — the shared Enter path for the keyboard
/// Enter arm and the mouse click-to-commit arm. Descends into a directory (incl. ".."),
/// guarding against unreadable targets, or opens a file through the dirty-guard path.
pub(crate) fn file_browser_enter(editor: &mut crate::editor::Editor) {
    let chosen = editor.file_browser.as_ref().and_then(|fb| {
        fb.entries.get(fb.selected).map(|e| (e.name.clone(), e.is_dir))
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
                // §3: check readability BEFORE committing fb.dir.
                if std::fs::read_dir(&target).is_ok() {
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.dir = target;
                        fb.query.clear();
                        fb.selected = 0;
                        fb.scroll_top = 0; // A6: reset with selected to avoid out-of-order slice
                        crate::file_browser::rebuild_entries(fb);
                    }
                } else {
                    editor.status = format!("cannot read directory: {}", target.display());
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
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.file_browser.is_none() { return crate::app::Handled::Pass(msg); }
    // Drop an async clipboard-paste result that arrives while the browser is open —
    // it must not land in the document behind the overlay (Codex I6, mirror palette).
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if let Msg::Input(Event::Paste(text)) = &msg {
        let ah = editor.active().view.area.1;
        if let Some(fb) = editor.file_browser.as_mut() {
            fb.query.push_str(text);
            crate::file_browser::rebuild_entries(fb);
            crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            match k.code {
                KeyCode::Esc => { editor.file_browser = None; }
                KeyCode::Enter => { file_browser_enter(editor); }
                c if crate::list_window::list_nav_key(c).is_some() => {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
                            ah, fb.entries.len(), &mut fb.selected, &mut fb.scroll_top);
                    }
                }
                KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.query.pop();
                        crate::file_browser::rebuild_entries(fb);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        fb.query.push(c);
                        crate::file_browser::rebuild_entries(fb);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    // Non-key msg falls through to normal handling while the browser stays open.
    crate::app::Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_entries_dirs_first_with_dotdot_and_filter() {
        let dir = std::env::temp_dir().join(format!("wc-fb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("alpha.md"), "x").unwrap();
        std::fs::write(dir.join("beta.txt"), "x").unwrap();
        let mut fb = FileBrowser { dir: dir.clone(), query: String::new(), entries: vec![], selected: 0, scroll_top: 0 };
        rebuild_entries(&mut fb);
        assert_eq!(fb.entries[0].name, "..", "parent first");
        let names: Vec<_> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        let sub_i = names.iter().position(|n| *n == "sub").unwrap();
        let alpha_i = names.iter().position(|n| *n == "alpha.md").unwrap();
        assert!(sub_i < alpha_i, "directories sort before files");
        fb.query = "alpha".into(); rebuild_entries(&mut fb);
        assert!(fb.entries.iter().any(|e| e.name == "alpha.md"));
        assert!(!fb.entries.iter().any(|e| e.name == "beta.txt"), "substring filter");
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
        e.open_file_browser(parent.clone());
        // Rebuild entries so "secret" appears in the list.
        if let Some(fb) = e.file_browser.as_mut() { rebuild_entries(fb); }
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
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &km, &ex, &clk, &tx);

        // Dir must NOT have changed — still at parent.
        let fb_dir = e.file_browser.as_ref().map(|fb| fb.dir.clone());
        assert_eq!(fb_dir.as_deref(), Some(parent.as_path()),
            "fb.dir must remain at parent after Enter on unreadable dir, got: {:?}", fb_dir);
        // Status must mention the unreadable directory.
        assert!(e.status.contains("cannot read directory"),
            "status must mention 'cannot read directory', got: {:?}", e.status);

        // Restore permissions so cleanup can remove the dir.
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn open_file_browser_enforces_xor() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 12));
        e.open_palette();
        e.open_file_browser(std::env::temp_dir());
        assert!(e.file_browser.is_some());
        assert!(e.palette.is_none(), "opening file_browser clears the palette (XOR)");
    }
}
