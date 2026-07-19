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
    #[serde(default)]
    pub marks: BTreeMap<String, usize>,
    /// On-disk mtime (seconds since epoch) at last persist.
    pub mtime: i64,
    /// On-disk file size at last persist.
    pub size: u64,
    /// Monotonic sequence number for LRU eviction. Higher = more recently used.
    pub seq: u64,
    /// 5g: folded heading byte-offsets. Defaulted so pre-5g session.toml loads.
    #[serde(default)]
    pub folds: Vec<usize>,
    /// 9a: persisted marked-block byte range [start, end). Defaulted so pre-9a session.toml loads.
    #[serde(default)]
    pub block: Option<(usize, usize)>,
}

/// Effort 6: the permanent *scratch* buffer's persisted content. Path-less, so it
/// cannot live in the path-keyed `entries` map — it is a sibling table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScratchState {
    pub text: String,
    pub cursor: usize,
}

/// Whole-session store. Keys are canonical absolute path strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub entries: BTreeMap<String, StateEntry>,
    /// Effort 6: scratch buffer content (sibling table; omitted when None so old
    /// readers and a never-used scratch don't emit an empty [scratch]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<ScratchState>,
}

impl SessionState {
    /// One past the highest stored seq — so a newly recorded entry outranks all
    /// loaded ones for LRU purposes (Codex pre-merge fix).
    pub fn next_seq(&self) -> u64 {
        self.entries.values().map(|e| e.seq).max().unwrap_or(0) + 1
    }

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
    /// Oversized scratch is dropped before writing; if metadata alone is still over cap,
    /// skip the write entirely (graceful). Propagates serialization errors.
    pub fn save_in(&self, dir: &Path) -> std::io::Result<()> {
        let mut text = toml::to_string(self)
            .map_err(|e| std::io::Error::other(format!("session serialize: {e}")))?;
        if text.len() > crate::limits::MAX_SESSION_BYTES {
            let trimmed = SessionState { scratch: None, ..self.clone() };
            text = toml::to_string(&trimmed)
                .map_err(|e| std::io::Error::other(format!("session serialize: {e}")))?;
            if text.len() > crate::limits::MAX_SESSION_BYTES {
                return Ok(()); // metadata alone over cap (shouldn't happen) → skip persist
            }
        }
        crate::file::save_atomic_bytes(&dir.join("session.toml"), text.as_bytes())
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Save to the real XDG state dir.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_in(&crate::swap::state_dir()?)
    }
}

/// Load from a specific directory (testable variant). Corrupt/missing/over-cap → empty.
/// The read is bounded at MAX_SESSION_BYTES+1 so a huge file is never fully slurped.
pub fn load_in(dir: &Path) -> SessionState {
    load_in_with_fs(&crate::fsx::RealFs, dir)
}

pub(crate) fn load_in_with_fs(fs: &dyn crate::fsx::Fs, dir: &Path) -> SessionState {
    let path = dir.join("session.toml");
    let cap = crate::limits::MAX_SESSION_BYTES as u64;
    let Ok(Some(bytes)) = fs.read_capped(&path, cap) else {
        return SessionState::default(); // missing, unreadable, or over-cap → empty (graceful)
    };
    let Ok(text) = String::from_utf8(bytes) else { return SessionState::default() };
    toml::from_str(&text).unwrap_or_default()
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
    file_identity_with_fs(&crate::fsx::RealFs, path)
}

/// Seam-taking core of [`file_identity`]. Kept `pub(crate)` so tests can inject a `FaultFs`.
pub(crate) fn file_identity_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<(i64, u64)> {
    let st = fs.stat(path).ok()?;
    // SAME guard as `save::fingerprint`, and for the same reason: today's
    // `std::fs::metadata(path).ok()?` FAILS for a broken symlink, so this returns None.
    // The seam's `stat` SUCCEEDS for one, so without this the session-restore staleness
    // check would receive a (mtime = 0, len = 0) identity — which matches nothing and
    // silently discards resume state, or worse matches a genuinely empty file.
    if st.broken { return None; }
    let mtime = st.mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((mtime, st.len))
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
                    folds: vec![],
                    block: None,
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
    fn next_seq_is_one_past_max() {
        let mut s = SessionState::default();
        assert_eq!(s.next_seq(), 1, "empty store → next_seq == 1");
        s.entries.insert("/a".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 0, size: 0, seq: 5, folds: vec![], block: None });
        s.entries.insert("/b".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 0, size: 0, seq: 9, folds: vec![], block: None });
        assert_eq!(s.next_seq(), 10, "entries at seq {{5,9}} → next_seq == 10");
    }

    #[test]
    fn fresh_entry_beats_old_entries_on_prune() {
        // Simulate: loaded session has entries at seq 5 and 9 (from a prior run).
        // A new entry recorded at seq 10 (from next_seq()) must survive; seq 5 is evicted.
        let mut s = SessionState::default();
        s.entries.insert("/old-a".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 0, size: 0, seq: 5, folds: vec![], block: None });
        s.entries.insert("/old-b".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 0, size: 0, seq: 9, folds: vec![], block: None });
        let new_seq = s.next_seq(); // == 10
        assert_eq!(new_seq, 10);
        s.record("/new".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 0, size: 0, seq: new_seq, folds: vec![], block: None }, 2);
        // Cap is 2: the freshest two must be /old-b (seq 9) and /new (seq 10); /old-a (seq 5) evicted.
        assert_eq!(s.entries.len(), 2, "pruned to cap");
        assert!(s.entries.contains_key("/new"), "newly-recorded entry must survive");
        assert!(s.entries.contains_key("/old-b"), "second-highest seq must survive");
        assert!(!s.entries.contains_key("/old-a"), "oldest entry (seq 5) must be evicted");
    }

    #[test]
    fn corrupt_state_file_loads_empty() {
        let dir = tmp();
        std::fs::write(dir.join("session.toml"), b"\xff not toml").unwrap();
        assert!(load_in(&dir).entries.is_empty());
    }

    #[test]
    fn old_session_toml_without_folds_loads_with_empty_folds() {
        // an entry serialized before `folds` existed must deserialize, not wipe.
        let toml = r#"
[entries."/tmp/x.md"]
cursor = 3
scroll = 0
mtime = 1
size = 2
seq = 1
"#;
        let s: SessionState = toml::from_str(toml).expect("must deserialize without folds");
        assert!(s.entries["/tmp/x.md"].folds.is_empty());
    }

    #[test]
    fn folds_round_trip_through_toml() {
        let mut s = SessionState::default();
        s.entries.insert("/tmp/x.md".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 1, size: 2, seq: 1, folds: vec![10, 42], block: None });
        let out = toml::to_string(&s).unwrap();
        let back: SessionState = toml::from_str(&out).unwrap();
        assert_eq!(back.entries["/tmp/x.md"].folds, vec![10, 42]);
    }

    #[test]
    fn old_session_toml_without_block_loads_with_none() {
        // An entry serialized before `block` existed must deserialize with block=None
        // (mirrors old_session_toml_without_folds_loads_with_empty_folds pattern).
        let toml = r#"
[entries."/tmp/x.md"]
cursor = 3
scroll = 0
mtime = 1
size = 2
seq = 1
"#;
        let s: SessionState = toml::from_str(toml).expect("must deserialize without block");
        assert!(s.entries["/tmp/x.md"].block.is_none(), "missing block key → None");
    }

    #[test]
    fn block_round_trips_through_toml() {
        let mut s = SessionState::default();
        s.entries.insert("/tmp/x.md".into(), StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(),
            mtime: 1, size: 2, seq: 1, folds: vec![],
            block: Some((10, 42)),
        });
        let out = toml::to_string(&s).unwrap();
        let back: SessionState = toml::from_str(&out).unwrap();
        assert_eq!(back.entries["/tmp/x.md"].block, Some((10, 42)));
    }

    #[test]
    fn scratch_state_round_trips_and_is_optional() {
        // Missing [scratch] → None.
        let s: SessionState = toml::from_str(r#"
[entries."/tmp/x.md"]
cursor = 1
scroll = 0
mtime = 1
size = 2
seq = 1
"#).unwrap();
        assert!(s.scratch.is_none(), "absent [scratch] → None");
        // Present round-trips and serializes as its own [scratch] table.
        let s2 = SessionState { scratch: Some(ScratchState { text: "stash\n\nmore".into(), cursor: 5 }), ..Default::default() };
        let out = toml::to_string(&s2).unwrap();
        assert!(out.contains("[scratch]"), "serializes as [scratch] table");
        let back: SessionState = toml::from_str(&out).unwrap();
        assert_eq!(back.scratch.unwrap().text, "stash\n\nmore");
    }

    #[test]
    fn save_in_drops_oversized_scratch_keeps_metadata() {
        let d = tmp();
        let mut s = SessionState::default();
        s.entries.insert("/a".into(), StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(),
            mtime: 1, size: 1, seq: 1, folds: vec![], block: None,
        });
        s.scratch = Some(ScratchState { text: "x".repeat(crate::limits::MAX_SESSION_BYTES + 1), cursor: 0 });
        s.save_in(&d).unwrap();
        let back = load_in(&d);
        assert!(back.scratch.is_none(), "oversized scratch dropped");
        assert!(back.entries.contains_key("/a"), "metadata still persisted");
    }

    #[test]
    fn load_in_over_cap_returns_empty() {
        let d = tmp();
        std::fs::write(d.join("session.toml"), "x".repeat(crate::limits::MAX_SESSION_BYTES + 1)).unwrap();
        assert!(load_in(&d).entries.is_empty(), "over-cap session.toml → empty");
    }

    #[cfg(unix)]
    #[test]
    fn file_identity_on_a_broken_symlink_is_none() {
        // Without the guard this returns Some((0, 0)) — an identity that silently discards
        // resume state, or matches a genuinely empty file.
        //
        // FAIL-VERIFY: delete the `if st.broken` line, watch this fail, then revert.
        let d = std::env::temp_dir().join(format!("wc-fid-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let link = d.join("dangling.md");
        std::os::unix::fs::symlink(d.join("gone.md"), &link).expect("symlink");
        assert!(file_identity_with_fs(&crate::fsx::RealFs, &link).is_none(),
            "a broken symlink must yield None, exactly as metadata().ok()? did");
        let _ = std::fs::remove_dir_all(&d);
    }
}
