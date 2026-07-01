//! Panic-time emergency buffer dump (spec §5.5). The unwind-path belt behind
//! the swap file's periodic protection.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Last-good snapshot, updated after each `apply`. The panic hook try_locks it.
pub static LAST_GOOD: Mutex<Option<(Option<PathBuf>, ropey::Rope)>> = Mutex::new(None);

/// Record the post-edit snapshot (O(1) rope clone). Called from `Editor::apply`.
pub fn record_snapshot(path: Option<&Path>, rope: ropey::Rope) {
    if let Ok(mut g) = LAST_GOOD.try_lock() {
        *g = Some((path.map(Path::to_path_buf), rope));
    }
}

/// Write a 0600 dump of `rope` into `dir`. Tested directly (no real panic).
pub fn write_dump(path: Option<&Path>, rope: &ropey::Rope, dir: &Path) -> std::io::Result<PathBuf> {
    let name = match path.and_then(|p| p.file_name()) {
        Some(n) => crate::swap::sanitize(&n.to_string_lossy()),
        None => "scratch".to_string(),
    };
    let out = dir.join(format!("recovered-{}-{}.md", name, std::process::id()));
    crate::swap::write_atomic(&out, &rope.to_string())?;
    Ok(out)
}

/// Dump every open buffer that holds unsaved work into `dir`, one 0600
/// `recovered-*.md` per buffer; returns how many were written. Used by the
/// input-loss shutdown (a controlled main-loop break, so iterating buffers is
/// safe — unlike the panic hook's conservative single try_lock `dump_on_panic`).
/// Uses raw `Document::dirty()` so a scratch buffer holding content is included
/// (its content is unsaved work); clean/empty buffers are skipped.
pub fn dump_all_dirty(editor: &crate::editor::Editor, dir: &Path) -> usize {
    let mut n = 0;
    for b in &editor.buffers {
        if b.document.dirty() {
            let rope = b.document.buffer.snapshot();
            if write_dump(b.document.path.as_deref(), &rope, dir).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Best-effort dump from the panic hook. `try_lock` (never block): a panic that
/// fired mid-update must not deadlock — skip the dump on contention.
pub fn dump_on_panic() {
    if let Ok(g) = LAST_GOOD.try_lock() {
        if let Some((path, rope)) = g.as_ref() {
            if let Ok(dir) = crate::swap::state_dir() {
                let _ = write_dump(path.as_deref(), rope, &dir);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn write_dump_writes_named_0600_file_with_body() {
        let dir = crate::swap::state_dir().unwrap();
        let rope = ropey::Rope::from_str("unsaved work\n");
        let out = write_dump(Some(Path::new("/home/u/notes.md")), &rope, &dir).unwrap();
        let name = out.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("recovered-notes.md-") && name.ends_with(".md"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "unsaved work\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn write_dump_handles_scratch_buffer() {
        let dir = crate::swap::state_dir().unwrap();
        let rope = ropey::Rope::from_str("scratch\n");
        let out = write_dump(None, &rope, &dir).unwrap();
        assert!(out.file_name().unwrap().to_string_lossy().starts_with("recovered-scratch-"));
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn dump_all_dirty_writes_one_file_per_dirty_buffer_including_scratch() {
        use crate::editor::{Buffer, Editor};
        use std::path::PathBuf;

        let dir = std::env::temp_dir().join(format!("wcartel-dumptest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // buffer 0: a CLEAN file buffer (path, never edited → version == saved_version).
        let mut e = Editor::new_from_text("clean\n", Some(PathBuf::from("/tmp/clean.md")), (80, 24));

        // a DIRTY file buffer (pushed, then version bumped past saved_version).
        let dirty_id = e.alloc_id();
        e.buffers.push(Buffer::from_text(dirty_id, "work\n", Some(PathBuf::from("/tmp/work.md")), (80, 24)));
        e.by_id_mut(dirty_id).unwrap().document.version = 1;

        // a DIRTY scratch buffer (pathless → dumped as recovered-scratch-*).
        e.install_scratch();
        let scratch_id = e.scratch_id.unwrap();
        e.by_id_mut(scratch_id).unwrap().document.version = 1;

        let n = dump_all_dirty(&e, &dir);
        assert_eq!(n, 2, "two dirty buffers dumped, clean one skipped");

        let names: Vec<String> = std::fs::read_dir(&dir).unwrap()
            .map(|x| x.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().any(|s| s.starts_with("recovered-scratch-")),
                "the scratch buffer with content is dumped");

        std::fs::remove_dir_all(&dir).ok();
    }
}
