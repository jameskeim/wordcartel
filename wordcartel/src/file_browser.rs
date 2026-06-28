//! File-browser overlay: lists the entries of a directory, filters by query,
//! navigates into directories (and `..`), and opens a file on selection.
//! Mirrors the theme picker (theme_picker.rs) / command palette.

use std::path::PathBuf;

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
        let mut fb = FileBrowser { dir: dir.clone(), query: String::new(), entries: vec![], selected: 0 };
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

    #[test]
    fn open_file_browser_enforces_xor() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 12));
        e.open_palette();
        e.open_file_browser(std::env::temp_dir());
        assert!(e.file_browser.is_some());
        assert!(e.palette.is_none(), "opening file_browser clears the palette (XOR)");
    }
}
