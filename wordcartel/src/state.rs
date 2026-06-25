// wordcartel/src/state.rs — path-keyed session state (resume-at-position + marks store).
//
// Persisted as $XDG_STATE_HOME/wordcartel/session.toml (same state_dir as swap).
// Keys are canonical absolute path strings. Staleness guard: mtime+size must match
// the file on disk at resume time; a mismatch → discard (never restore stale state).

use std::collections::BTreeMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

/// Per-file session state. `marks` keys are single-char Strings (NOT char —
/// toml rejects non-string map keys; the mark/jump commands in 5c convert at
/// the app boundary).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateEntry {
    pub cursor: usize,
    pub scroll: usize,
    /// Mark name → byte offset. Keys are single-char Strings (toml constraint).
    pub marks: BTreeMap<String, usize>,
    /// On-disk mtime (seconds since epoch) at last persist.
    pub mtime: i64,
    /// On-disk file size at last persist.
    pub size: u64,
    /// Monotonic sequence number for LRU eviction. Higher = more recently used.
    pub seq: u64,
}

/// Whole-session store. Keys are canonical absolute path strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub entries: BTreeMap<String, StateEntry>,
}

impl SessionState {
    /// Insert `entry` for `path`, then evict the lowest-`seq` entries beyond
    /// `max_entries` (LRU by seq).
    pub fn record(&mut self, path: String, entry: StateEntry, max_entries: usize) {
        self.entries.insert(path, entry);
        while self.entries.len() > max_entries {
            // Find the key with the minimum seq and evict it.
            if let Some(k) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.seq)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&k);
            } else {
                break;
            }
        }
    }

    /// Serialize to TOML and write atomically to `dir/session.toml`.
    /// Propagates serialization errors — does NOT silently drop state.
    pub fn save_in(&self, dir: &Path) -> std::io::Result<()> {
        let text = toml::to_string(self).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("session serialize: {e}"),
            )
        })?;
        crate::file::save_atomic_bytes(&dir.join("session.toml"), text.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Save to the real XDG state dir.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_in(&crate::swap::state_dir()?)
    }
}

/// Load from a specific directory (testable variant). Corrupt/missing → empty.
pub fn load_in(dir: &Path) -> SessionState {
    match std::fs::read_to_string(dir.join("session.toml")) {
        Ok(t) => toml::from_str(&t).unwrap_or_default(),
        Err(_) => SessionState::default(),
    }
}

/// Load from the real XDG state dir. Corrupt/missing → empty.
pub fn load() -> SessionState {
    match crate::swap::state_dir() {
        Ok(d) => load_in(&d),
        Err(_) => SessionState::default(),
    }
}

/// Return the (mtime_secs, size) identity of a file, or None on error.
pub fn file_identity(path: &Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((mtime, m.len()))
}

// ---------------------------------------------------------------------------
// Tests — written first (RED phase), then turned GREEN by implementation above
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "wc-state-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn round_trip_and_prune_lru() {
        let dir = tmp();
        let mut s = SessionState::default();
        for i in 0..5u64 {
            s.record(
                format!("/f{i}"),
                StateEntry {
                    cursor: i as usize,
                    scroll: 0,
                    marks: Default::default(),
                    mtime: 1,
                    size: 1,
                    seq: i,
                },
                3, // cap 3
            );
        }
        assert_eq!(s.entries.len(), 3, "LRU-pruned to cap");
        assert!(s.entries.contains_key("/f4") && !s.entries.contains_key("/f0"));
        s.save_in(&dir).unwrap();
        let back = load_in(&dir);
        assert_eq!(back.entries.len(), 3);
        assert_eq!(back.entries["/f4"].cursor, 4);
    }

    #[test]
    fn corrupt_state_file_loads_empty() {
        let dir = tmp();
        std::fs::write(dir.join("session.toml"), b"\xff not toml").unwrap();
        assert!(load_in(&dir).entries.is_empty());
    }
}
