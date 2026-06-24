//! Swap / recovery file (spec §5.1). Atomic, 0600, header+body snapshot in the
//! 0700 XDG state dir. Never writes the user's .md.

use std::io;
use std::path::{Path, PathBuf};

/// FNV-1a 64-bit — stable across Rust versions (unlike DefaultHasher), no dep.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Replace path separators and unusual chars so the name is a safe single
/// filename component.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// `$XDG_STATE_HOME/wordcartel`, created 0700 on Unix. Falls back to
/// `~/.local/state/wordcartel` when `dirs::state_dir()` is None.
pub fn state_dir() -> io::Result<PathBuf> {
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state dir"))?;
    let dir = base.join("wordcartel");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

pub const FORMAT: &str = "wcartel-swap 1";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SwapHeader {
    pub realpath: Option<String>,
    pub load_mtime_secs: Option<u64>,
    pub load_size: Option<u64>,
    pub content_hash: u64,
    pub version: u64,
    pub ts_ms: u64,
    pub pid: u32,
}

fn opt_str(s: &Option<String>) -> String { s.clone().unwrap_or_else(|| "-".into()) }
fn opt_u64(n: Option<u64>) -> String { n.map(|x| x.to_string()).unwrap_or_else(|| "-".into()) }

pub fn serialize(h: &SwapHeader, body: &str) -> String {
    format!(
        "{FORMAT}\npath: {}\nfp: {}:{}\nhash: {:016x}\nversion: {}\nts: {}\npid: {}\n---\n{}",
        opt_str(&h.realpath),
        opt_u64(h.load_mtime_secs),
        opt_u64(h.load_size),
        h.content_hash, h.version, h.ts_ms, h.pid, body,
    )
}

pub fn parse(text: &str) -> Option<(SwapHeader, String)> {
    let (head, body) = text.split_once("\n---\n")?;
    let mut lines = head.lines();
    if lines.next()? != FORMAT { return None; }
    let mut realpath = None;
    let mut load_mtime_secs = None;
    let mut load_size = None;
    let mut content_hash = None;
    let mut version = None;
    let mut ts_ms = None;
    let mut pid = None;
    for line in lines {
        let (k, v) = line.split_once(": ")?;
        match k {
            "path" => realpath = if v == "-" { None } else { Some(v.to_string()) },
            "fp" => {
                let (m, s) = v.split_once(':')?;
                load_mtime_secs = if m == "-" { None } else { Some(m.parse().ok()?) };
                load_size = if s == "-" { None } else { Some(s.parse().ok()?) };
            }
            "hash" => content_hash = Some(u64::from_str_radix(v, 16).ok()?),
            "version" => version = Some(v.parse().ok()?),
            "ts" => ts_ms = Some(v.parse().ok()?),
            "pid" => pid = Some(v.parse().ok()?),
            _ => {}
        }
    }
    Some((
        SwapHeader {
            realpath,
            load_mtime_secs,
            load_size,
            content_hash: content_hash?,
            version: version?,
            ts_ms: ts_ms?,
            pid: pid?,
        },
        body.to_string(),
    ))
}

/// Per-document swap path. Named docs hash their realpath (best-effort canonical)
/// to disambiguate same-named files; scratch buffers key on pid.
pub fn swap_path(doc_path: Option<&Path>) -> io::Result<PathBuf> {
    let dir = state_dir()?;
    let name = match doc_path {
        Some(p) => {
            let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let h = fnv1a64(real.to_string_lossy().as_bytes());
            format!("{}-{:016x}.swp", sanitize(&base), h)
        }
        None => format!("scratch-{}.swp", std::process::id()),
    };
    Ok(dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn fnv_is_stable_and_distinguishes() {
        assert_eq!(fnv1a64(b"abc"), fnv1a64(b"abc"));
        assert_ne!(fnv1a64(b"/a/notes.md"), fnv1a64(b"/b/notes.md"));
    }

    #[test]
    fn sanitize_strips_separators() {
        assert_eq!(sanitize("a/b c.md"), "a_b_c.md");
        assert!(!sanitize("../x").contains('/'));
    }

    #[test]
    fn swap_path_named_is_deterministic_and_in_state_dir() {
        let a = swap_path(Some(Path::new("/home/u/notes.md"))).unwrap();
        let b = swap_path(Some(Path::new("/home/u/notes.md"))).unwrap();
        assert_eq!(a, b, "same doc → same swap path");
        assert!(a.file_name().unwrap().to_string_lossy().ends_with(".swp"));
        assert!(a.starts_with(state_dir().unwrap()));
    }

    #[test]
    fn swap_path_scratch_uses_pid() {
        let s = swap_path(None).unwrap();
        let name = s.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("scratch-") && name.ends_with(".swp"));
    }

    #[cfg(unix)]
    #[test]
    fn state_dir_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let d = state_dir().unwrap();
        let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "state dir must be owner-only");
    }

    #[test]
    fn header_round_trips() {
        let h = SwapHeader {
            realpath: Some("/home/u/notes.md".into()),
            load_mtime_secs: Some(1_700_000_000),
            load_size: Some(42),
            content_hash: fnv1a64(b"body text\n"),
            version: 7,
            ts_ms: 1_700_000_123_456,
            pid: 4321,
        };
        let body = "body text\n";
        let text = serialize(&h, body);
        let (h2, body2) = parse(&text).expect("must parse");
        assert_eq!(h2, h);
        assert_eq!(body2, body);
    }

    #[test]
    fn scratch_header_round_trips_with_none_fields() {
        let h = SwapHeader {
            realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"x"), version: 1, ts_ms: 5, pid: 9,
        };
        let (h2, b2) = parse(&serialize(&h, "x")).unwrap();
        assert_eq!(h2, h);
        assert_eq!(b2, "x");
    }

    #[test]
    fn parse_rejects_unknown_format() {
        assert!(parse("garbage\nwith no header\n").is_none());
    }
}
