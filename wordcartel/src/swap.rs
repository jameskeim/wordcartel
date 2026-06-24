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

pub const T_IDLE_MS: u64 = 2_000;
pub const T_MAX_MS: u64 = 30_000;

/// Is a swap write due now? Requires a prior edit (the caller also gates on
/// `editor.document.dirty()`); fires on idle-debounce OR max-cap.
pub fn due(now: u64, last_edit_at: Option<u64>, last_swap_at: Option<u64>) -> bool {
    let Some(edit) = last_edit_at else { return false };
    let idle_due = now.saturating_sub(edit) >= T_IDLE_MS;
    let max_due = match last_swap_at {
        Some(swap) => now.saturating_sub(swap) >= T_MAX_MS,
        None => now.saturating_sub(edit) >= T_MAX_MS, // never swapped since first edit
    };
    idle_due || max_due
}

/// The next instant the loop should wake to consider a swap (for recv_timeout).
pub fn next_deadline_ms(now: u64, last_edit_at: Option<u64>, last_swap_at: Option<u64>) -> Option<u64> {
    let edit = last_edit_at?;
    let idle_at = edit.saturating_add(T_IDLE_MS);
    let max_at = last_swap_at.unwrap_or(edit).saturating_add(T_MAX_MS);
    Some(idle_at.min(max_at).max(now))
}

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

/// Best-effort delete of a document's swap file.
pub fn delete(doc_path: Option<&Path>) {
    if let Ok(p) = swap_path(doc_path) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(target_os = "linux")]
fn pid_is_live(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
#[cfg(not(target_os = "linux"))]
fn pid_is_live(_pid: u32) -> bool {
    false // best-effort elsewhere: treat as not-live → offer recovery
}

/// Find an orphaned scratch swap from a previous (non-live) process, if any.
/// Scratch swaps are pid-keyed; after a crash the new session won't find its
/// own. Skip our own pid and live pids; return the newest valid non-empty
/// orphan as (file path, header, body).
pub fn find_orphan_scratch_swap() -> Option<(std::path::PathBuf, SwapHeader, String)> {
    find_orphan_scratch_swap_in(&state_dir().ok()?)
}

/// Dir-injectable core of `find_orphan_scratch_swap` so tests can isolate from the
/// shared real state dir (which accumulates orphan litter across runs).
fn find_orphan_scratch_swap_in(dir: &std::path::Path) -> Option<(std::path::PathBuf, SwapHeader, String)> {
    let me = std::process::id();
    let mut best: Option<(std::path::PathBuf, SwapHeader, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        let pid = fname.strip_prefix("scratch-")
            .and_then(|s| s.strip_suffix(".swp"))
            .and_then(|s| s.parse::<u32>().ok());
        let Some(pid) = pid else { continue };
        if pid == me || pid_is_live(pid) { continue; }
        let raw = match std::fs::read_to_string(entry.path()) { Ok(s) => s, Err(_) => continue };
        let Some((header, body)) = parse(&raw) else { continue };
        if body.is_empty() { continue; }
        let newer = match &best { Some((_, h, _)) => header.ts_ms > h.ts_ms, None => true };
        if newer { best = Some((entry.path(), header, body)); }
    }
    best
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

#[derive(Debug)]
pub enum RecoveryDecision {
    OpenNormally,
    DiscardSilently,
    Prompt(SwapHeader, String),
}

/// Recovery predicate (spec §5.1): content-hash first, stat as tiebreaker.
/// `current_file_bytes` is `Some` when the doc path exists on disk, else `None`.
pub fn assess(doc_path: Option<&Path>, current_file_bytes: Option<&[u8]>) -> RecoveryDecision {
    let sp = match swap_path(doc_path) { Ok(p) => p, Err(_) => return RecoveryDecision::OpenNormally };
    let raw = match std::fs::read_to_string(&sp) { Ok(s) => s, Err(_) => return RecoveryDecision::OpenNormally };
    let (header, body) = match parse(&raw) {
        Some(x) => x,
        None => return RecoveryDecision::Prompt(
            // Unparseable swap of unknown provenance → let the user decide.
            SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
                content_hash: 0, version: 0, ts_ms: 0, pid: 0 },
            String::new(),
        ),
    };
    match current_file_bytes {
        Some(bytes) if header.content_hash == fnv1a64(bytes) => RecoveryDecision::DiscardSilently,
        _ => RecoveryDecision::Prompt(header, body), // diverged, missing F, or scratch
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
            let ok = write_atomic(&path, &serialize(&h, &body)).is_ok();
            JobResult {
                version,
                kind: JobKind::SwapWrite,
                merge: Box::new(move |editor| {
                    editor.swap_in_flight = false;
                    if ok {
                        editor.last_swap_at = Some(ts);
                    } else {
                        editor.status = "swap write failed".to_string();
                    }
                }),
            }
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

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
    fn cadence_idle_debounce_fires_after_t_idle() {
        // Edited at 1000, never swapped. At 1000+T_idle it is due.
        assert!(!due(1000 + T_IDLE_MS - 1, Some(1000), None));
        assert!(due(1000 + T_IDLE_MS, Some(1000), None));
    }

    #[test]
    fn cadence_max_cap_fires_during_continuous_editing() {
        // Continuous editing: last_edit keeps moving so idle never elapses, but
        // last_swap is old → max-cap forces a write.
        let now = 100_000;
        assert!(due(now, Some(now), Some(now - T_MAX_MS)));      // max elapsed
        assert!(!due(now, Some(now), Some(now - T_MAX_MS + 1))); // not yet
    }

    #[test]
    fn cadence_not_due_when_never_edited() {
        assert!(!due(99_999, None, None));
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
    fn recovery_no_swap_opens_normally() {
        // A doc path whose swap file does not exist.
        let p = std::env::temp_dir().join(format!("wc-norec-{}.md", std::process::id()));
        let _ = std::fs::remove_file(swap_path(Some(&p)).unwrap());
        assert!(matches!(assess(Some(&p), Some(b"abc\n")), RecoveryDecision::OpenNormally));
    }

    #[test]
    fn recovery_hash_equal_discards_silently() {
        let p = std::env::temp_dir().join(format!("wc-eq-{}.md", std::process::id()));
        let body = "same\n";
        let h = SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(body.as_bytes()), version: 1, ts_ms: 1, pid: 1 };
        write_atomic(&swap_path(Some(&p)).unwrap(), &serialize(&h, body)).unwrap();
        // F on disk == swap body → swap adds nothing.
        assert!(matches!(assess(Some(&p), Some(body.as_bytes())), RecoveryDecision::DiscardSilently));
        let _ = std::fs::remove_file(swap_path(Some(&p)).unwrap());
    }

    #[test]
    fn recovery_diverged_prompts() {
        let p = std::env::temp_dir().join(format!("wc-div-{}.md", std::process::id()));
        let body = "swap version\n";
        let h = SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(body.as_bytes()), version: 9, ts_ms: 1, pid: 1 };
        write_atomic(&swap_path(Some(&p)).unwrap(), &serialize(&h, body)).unwrap();
        // F differs from swap → prompt, carrying the swap body for Recover.
        match assess(Some(&p), Some(b"file version\n")) {
            RecoveryDecision::Prompt(hdr, b) => { assert_eq!(hdr.version, 9); assert_eq!(b, body); }
            other => panic!("expected Prompt, got {other:?}"),
        }
        let _ = std::fs::remove_file(swap_path(Some(&p)).unwrap());
    }

    #[test]
    fn find_orphan_scratch_swap_finds_dead_pid_and_skips_self() {
        // Write an orphan scratch swap with a fake dead pid (999999 is unreachable
        // in practice; pid_is_live returns false for it on Linux since /proc/999999
        // won't exist unless the system is truly overloaded — we also check).
        // Use a UNIQUE temp dir, not the shared real state dir: the finder returns
        // the newest orphan across the whole dir, and the real state dir accumulates
        // scratch-*.swp litter from other runs that would outrank our planted file.
        let dir = std::env::temp_dir().join(format!(
            "wc-orphan-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fake_pid: u32 = 999_999;
        // On Linux, verify the fake pid is indeed not live before depending on it.
        #[cfg(target_os = "linux")]
        assert!(!pid_is_live(fake_pid), "test invariant: pid 999999 must not be live");

        let orphan_path = dir.join(format!("scratch-{fake_pid}.swp"));
        let my_path = dir.join(format!("scratch-{}.swp", std::process::id()));

        // Write the orphan with non-empty body.
        let h = SwapHeader {
            realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"orphaned text\n"), version: 1, ts_ms: 42_000, pid: fake_pid,
        };
        write_atomic(&orphan_path, &serialize(&h, "orphaned text\n")).unwrap();

        // Also write a swap for our own pid — finder must skip it.
        let my_h = SwapHeader {
            realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"self text\n"), version: 1, ts_ms: 43_000,
            pid: std::process::id(),
        };
        write_atomic(&my_path, &serialize(&my_h, "self text\n")).unwrap();

        let result = find_orphan_scratch_swap_in(&dir);
        assert!(result.is_some(), "finder must return the orphan");
        let (found_path, found_header, found_body) = result.unwrap();
        assert_eq!(found_path, orphan_path, "finder must return the dead-pid orphan");
        assert_eq!(found_header.pid, fake_pid);
        assert_eq!(found_body, "orphaned text\n");
        // The self-pid swap must NOT have been returned.
        assert_ne!(found_path, my_path, "finder must not return our own pid's swap");

        let _ = std::fs::remove_file(&orphan_path);
        let _ = std::fs::remove_file(&my_path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispatch_swap_write_writes_a_recoverable_swap() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 123 } }

        let doc_path = std::env::temp_dir().join(format!(
            "wc-dispatch-swap-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let mut e = Editor::new_from_text("swap me\n", Some(doc_path.clone()), (80, 24));
        e.document.version = 3;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
          dispatch_swap_write(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert_eq!(e.last_swap_at, Some(123), "merge records last_swap_at");
        let sp = swap_path(Some(&doc_path)).unwrap();
        let (h, body) = parse(&std::fs::read_to_string(&sp).unwrap()).unwrap();
        assert_eq!(body, "swap me\n");
        assert_eq!(h.version, 3);
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }
}
