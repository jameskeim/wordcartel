//! Swap / recovery file (spec §5.1). Atomic, 0600, header+body snapshot in the
//! 0700 XDG state dir. Never writes the user's .md.

use crate::editor::Editor;
use crate::jobs::{Job, JobKind, JobResult};
use crate::registry::Ctx;
use std::io;
use std::io::Write as _;
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

/// Atomic 0600 write into our own state dir (no symlink/skip-unchanged logic).
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut f = open_excl_0600(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn open_excl_0600(p: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p)
}
#[cfg(not(unix))]
fn open_excl_0600(p: &Path) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(p)
}

pub fn build_header(editor: &Editor, body: &str, ts_ms: u64) -> SwapHeader {
    let realpath = editor.document.path.as_ref().map(|p| {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned()
    });
    let (load_mtime_secs, load_size) = match editor.document.stored_fp {
        Some(fp) => (
            fp.mtime.and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            Some(fp.size),
        ),
        None => (None, None),
    };
    SwapHeader {
        realpath,
        load_mtime_secs,
        load_size,
        content_hash: fnv1a64(body.as_bytes()),
        version: editor.document.version,
        ts_ms,
        pid: std::process::id(),
    }
}

/// Dispatch a SwapWrite job: capture an O(1) snapshot + header inputs now;
/// materialize + write on the worker; the merge records last_swap_at.
pub fn dispatch_swap_write(ctx: &mut Ctx) {
    let path = match swap_path(ctx.editor.document.path.as_deref()) {
        Ok(p) => p,
        Err(_) => return, // no state dir → best-effort; skip silently
    };
    let snap = ctx.editor.document.buffer.snapshot();
    let ts = ctx.clock.now_ms();
    let header = build_header(ctx.editor, "", ts); // body filled on worker
    let version = ctx.editor.document.version;
    ctx.executor.dispatch(Job {
        version,
        kind: JobKind::SwapWrite,
        run: Box::new(move || {
            let body = snap.to_string();
            let mut h = header;
            h.content_hash = fnv1a64(body.as_bytes());
            let _ = write_atomic(&path, &serialize(&h, &body)); // best-effort
            JobResult {
                version,
                kind: JobKind::SwapWrite,
                merge: Box::new(move |editor| { editor.last_swap_at = Some(ts); }),
            }
        }),
    });
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
    fn body_containing_separator_round_trips() {
        // A body that itself contains a "\n---\n" line must round-trip: split_once
        // takes the FIRST match (the header boundary), leaving the body intact.
        let h = SwapHeader {
            realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"first\n---\nsecond\n"),
            version: 1, ts_ms: 1, pid: 1,
        };
        let body = "first\n---\nsecond\n";
        let (h2, b2) = parse(&serialize(&h, body)).expect("must parse");
        assert_eq!(h2, h);
        assert_eq!(b2, body, "body containing the separator must survive round-trip");
    }

    #[test]
    fn parse_rejects_unknown_format() {
        assert!(parse("garbage\nwith no header\n").is_none());
    }

    #[test]
    fn write_atomic_writes_0600_and_roundtrips_via_parse() {
        let dir = state_dir().unwrap();
        let p = dir.join(format!("test-write-{}.swp", std::process::id()));
        let h = SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"hello\n"), version: 1, ts_ms: 1, pid: 1 };
        write_atomic(&p, &serialize(&h, "hello\n")).unwrap();
        let back = std::fs::read_to_string(&p).unwrap();
        let (h2, body) = parse(&back).unwrap();
        assert_eq!(h2.content_hash, h.content_hash);
        assert_eq!(body, "hello\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "swap file must be owner-only");
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dispatch_swap_write_writes_a_recoverable_swap() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 123 } }

        let mut e = Editor::new_from_text("swap me\n", None, (80, 24)); // scratch
        e.document.version = 3;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
          dispatch_swap_write(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert_eq!(e.last_swap_at, Some(123), "merge records last_swap_at");
        let sp = swap_path(None).unwrap();
        let (h, body) = parse(&std::fs::read_to_string(&sp).unwrap()).unwrap();
        assert_eq!(body, "swap me\n");
        assert_eq!(h.version, 3);
        let _ = std::fs::remove_file(&sp);
    }
}
