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
}
