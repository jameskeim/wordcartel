//! Swap / recovery file (spec §5.1). Atomic, 0600, header+body snapshot in the
//! 0700 XDG state dir. Never writes the user's .md.

use crate::editor::Editor;
use crate::jobs::{Job, JobKind, JobResult, ResultClass};
use crate::registry::Ctx;
use std::collections::HashSet;
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

/// True iff the buffer holds unsaved content whose version is not yet captured in the swap
/// file — the real "swap work pending" signal. This, NOT "an edit ever happened", gates both
/// the main-loop wake-up and the Tick dispatch. `last_edit_at` is monotonic (set on every edit,
/// never cleared or advanced by a swap), so basing the wake on it made `next_deadline_ms` return
/// a permanently past-due instant. The measured effect once idle: while DIRTY the swap file was
/// rewritten on every loop wake (continuous disk I/O + fsyncs, at ~0% userspace CPU — an SSD-wear /
/// no-idle-heat pathology); while SAVED-and-idle the loop kept waking on the past-due deadline with
/// no work to do. `swapped_version` is set to the written version when a swap succeeds, so once the
/// current version is on disk (or the buffer is clean) this is `false` and the loop can block.
pub fn pending(dirty: bool, version: u64, swapped_version: Option<u64>) -> bool {
    dirty && swapped_version != Some(version)
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
pub(crate) fn pid_is_live(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn pid_is_live(_pid: u32) -> bool {
    false // best-effort elsewhere: treat as not-live → offer recovery
}

/// Read a swap file, refusing (None) if it exceeds the cap — never slurp unbounded.
fn read_swap_capped(fs: &dyn crate::fsx::Fs, path: &std::path::Path) -> Option<String> {
    let bytes = fs.read_capped(path, crate::limits::MAX_OPEN_BYTES).ok()??;
    String::from_utf8(bytes).ok()
}

/// Read a file as raw bytes, refusing (None) if it exceeds the cap or is unreadable — the
/// byte-exact counterpart to `read_swap_capped` (a user's saved file need not be UTF-8, and
/// the `.tmp`/`assess` comparisons are byte-for-byte). Never slurps unbounded.
fn read_file_capped_bytes(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<Vec<u8>> {
    fs.read_capped(path, crate::limits::MAX_OPEN_BYTES).ok()?
}

/// The swap paths this session must NEVER offer for cleaning: every open buffer's swap (named
/// or scratch) plus this session's own scratch swap. Consumed by `cleanable_recovery_files`.
pub(crate) fn open_swap_paths(editor: &Editor) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let Ok(p) = swap_path(None) { set.insert(p); } // this session's own scratch swap
    for b in &editor.buffers {
        if let Ok(p) = swap_path(b.document.path.as_deref()) { set.insert(p); }
    }
    set
}

/// Enumerate the recovery artifacts in `dir` that are PROVABLY safe to delete — the single
/// source of truth for the `clean_recovery` command (H5). It FAILS CLOSED: any file whose
/// recovery value cannot be positively disproved is EXCLUDED. `protected` holds swap paths
/// that must never be offered (open buffers' swaps + this session's own scratch — see
/// `open_swap_paths`). The command snapshots the returned `Vec` and deletes exactly that set,
/// so this never itself removes anything.
///
/// Inclusion rules (everything else is excluded):
/// * `recovered-*.md` — the app's own already-extracted recovery dump; the user is explicitly
///   clearing it via this named command.
/// * `*.swp` — ONLY when its header parses, its `realpath` is `Some`, the writing pid is not
///   live, the candidate path is EXACTLY `swap_path(realpath)` (binding the verdict to THIS
///   file — never a relocated/stale twin), and `assess(realpath, <saved bytes>)` returns
///   `DiscardSilently` (swap body == the saved file → zero recovery value). `Prompt`,
///   `OpenNormally`, unreadable, unparseable, or `realpath = None` → EXCLUDED.
/// * `.tmp` — an atomic-write temp, ONLY when its target file (same dir, name recovered from
///   the temp) exists and is byte-identical (the temp merely duplicates an already-committed
///   file). A live/own writing pid, a missing target, or ANY divergence → EXCLUDED (a
///   crash-window temp can hold the newest, only snapshot).
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &HashSet<PathBuf>) -> Vec<PathBuf>
{
    let me = std::process::id();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if recovery_file_is_cleanable(fs, dir, &path, &fname, protected, me) { out.push(path); }
    }
    out
}

/// The single per-path cleanability verdict shared by `cleanable_recovery_files` (enumeration)
/// and `recovery_path_still_cleanable` (H5 confirm-time re-verify). `dir` is the directory the
/// candidate lives in (needed to resolve a `.tmp` target); `me` is our pid. Fails closed: any
/// path whose recovery value cannot be positively disproved returns `false`.
fn recovery_file_is_cleanable(
    fs: &dyn crate::fsx::Fs, dir: &Path, path: &Path, fname: &str,
    protected: &HashSet<PathBuf>, me: u32,
) -> bool {
    if protected.contains(path) { return false; } // open buffer / session swap → never offer
    if fname.starts_with("recovered-") && fname.ends_with(".md") {
        true                                       // the app's own already-extracted dump
    } else if fname.ends_with(".swp") {
        swap_is_cleanable(fs, path)
    } else if fname.ends_with(".tmp") {
        tmp_is_cleanable(fs, dir, path, fname, me)
    } else {
        false
    }
}

/// H5 confirm-time re-verify (inverse-TOCTOU hardening): re-run the enumerator's exact per-path
/// oracle for ONE snapshotted path, so a swap/temp whose CONTENT became recoverable while the
/// confirm modal was open is skipped instead of deleted. The snapshot remains the ceiling — this
/// only ever narrows it — so the forward-TOCTOU law (never sweep a file that appeared after the
/// prompt) is untouched. `protected` is gathered exactly as enumeration did (`open_swap_paths`).
/// Fails closed: a path with no parent dir or file name is treated as no-longer-cleanable.
pub(crate) fn recovery_path_still_cleanable(fs: &dyn crate::fsx::Fs, path: &Path,
    protected: &HashSet<PathBuf>) -> bool
{
    let (Some(dir), Some(fname)) = (path.parent(), path.file_name()) else { return false };
    recovery_file_is_cleanable(fs, dir, path, &fname.to_string_lossy(), protected, std::process::id())
}

/// True iff a `*.swp` candidate is provably valueless (see `cleanable_recovery_files`).
/// Every early return is a "keep" (fail closed).
fn swap_is_cleanable(fs: &dyn crate::fsx::Fs, candidate: &Path) -> bool {
    let Some(raw) = read_swap_capped(fs, candidate) else { return false };  // unreadable / oversized
    let Some((header, _body)) = parse(&raw) else { return false };      // unparseable provenance
    if pid_is_live(header.pid) { return false }                         // a live writer owns it
    let Some(rp) = header.realpath.as_deref() else { return false };    // no doc path recorded
    let real = Path::new(rp);
    // Bind the verdict to THIS file: `assess` recomputes `swap_path(realpath)` and judges
    // whatever lives there. Require the candidate to BE that canonical swap, else a clean
    // verdict for a twin could wrongly greenlight deleting a DIFFERENT, recoverable swap.
    match swap_path(Some(real)) {
        Ok(canonical) if canonical == *candidate => {}
        _ => return false,
    }
    let current = read_file_capped_bytes(fs, real);
    matches!(assess(fs, Some(real), current.as_deref()), RecoveryDecision::DiscardSilently)
}

/// True iff a `.tmp` atomic-write temp is provably valueless (see `cleanable_recovery_files`).
/// Name shape (fsx::create_temp): `.{target}.wcartel-{pid}-{counter}.tmp`.
fn tmp_is_cleanable(fs: &dyn crate::fsx::Fs, dir: &Path, candidate: &Path, fname: &str, me: u32) -> bool {
    let Some(stripped) = fname.strip_prefix('.') else { return false };
    let Some((target_name, tail)) = stripped.rsplit_once(".wcartel-") else { return false };
    let Some(ids) = tail.strip_suffix(".tmp") else { return false };
    let Some(pid) = ids.split('-').next().and_then(|s| s.parse::<u32>().ok()) else { return false };
    if pid == me || pid_is_live(pid) { return false }  // our own / a live writer's temp
    if target_name.is_empty() { return false }
    let target = dir.join(target_name);
    // Delete ONLY a temp that byte-duplicates its already-committed target; a missing target
    // (crash before any rename) or any divergence means the temp may be the only/newest copy.
    match (read_file_capped_bytes(fs, &target), read_file_capped_bytes(fs, candidate)) {
        (Some(t), Some(c)) => t == c,
        _ => false,
    }
}

/// Find an orphaned scratch swap from a previous (non-live) process, if any.
/// Scratch swaps are pid-keyed; after a crash the new session won't find its
/// own. Skip our own pid and live pids; return the newest valid non-empty
/// orphan as (file path, header, body).
pub fn find_orphan_scratch_swap() -> Option<(std::path::PathBuf, SwapHeader, String)> {
    find_orphan_scratch_swap_in(&crate::fsx::RealFs, &state_dir().ok()?)
}

/// Dir-injectable core of `find_orphan_scratch_swap` so tests can isolate from the
/// shared real state dir (which accumulates orphan litter across runs).
fn find_orphan_scratch_swap_in(fs: &dyn crate::fsx::Fs, dir: &std::path::Path)
    -> Option<(std::path::PathBuf, SwapHeader, String)>
{
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
        let Some(raw) = read_swap_capped(fs, &entry.path()) else { continue };
        let Some((header, body)) = parse(&raw) else { continue };
        if body.is_empty() { continue; }
        let newer = match &best { Some((_, h, _)) => header.ts_ms > h.ts_ms, None => true };
        if newer { best = Some((entry.path(), header, body)); }
    }
    best
}

/// Atomic 0600 write into our own state dir (no symlink/skip-unchanged logic, no
/// dir-fsync). Routes through the shared fault-tested core, inheriting its
/// TempGuard cleanup (this path previously left a temp behind on write failure).
/// Thin `RealFs` wrapper for callers with no `fs` in scope (notably
/// `recovery::write_dump`, which runs from the panic hook).
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    write_atomic_with_fs(&crate::fsx::RealFs, path, content)
}

pub(crate) fn write_atomic_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &str)
    -> io::Result<()>
{
    crate::fsx::atomic_replace(fs, path, content.as_bytes(), crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::Fixed(0o600), dir_fsync: false,
    })
}

pub fn build_header(editor: &Editor, body: &str, ts_ms: u64) -> SwapHeader {
    let realpath = editor.active().document.path.as_ref().map(|p| {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned()
    });
    let (load_mtime_secs, load_size) = match editor.active().document.stored_fp {
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
        version: editor.active().document.version,
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
pub fn assess(fs: &dyn crate::fsx::Fs, doc_path: Option<&Path>, current_file_bytes: Option<&[u8]>)
    -> RecoveryDecision
{
    let sp = match swap_path(doc_path) { Ok(p) => p, Err(_) => return RecoveryDecision::OpenNormally };
    let Some(raw) = read_swap_capped(fs, &sp) else { return RecoveryDecision::OpenNormally };
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
    let path = match swap_path(ctx.editor.active().document.path.as_deref()) {
        Ok(p) => p,
        Err(_) => return, // no state dir → best-effort; skip silently
    };
    let snap = ctx.editor.active().document.buffer.snapshot();
    let ts = ctx.clock.now_ms();
    let header = build_header(ctx.editor, "", ts); // body filled on worker
    let version = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;
    let fs = std::sync::Arc::clone(&ctx.fs);   // owned — the closure is 'static + Send
    ctx.executor.dispatch(Job {
        buffer_id,
        class: ResultClass::Durability,
        version,
        kind: JobKind::SwapWrite,
        run: Box::new(move || {
            let body = snap.to_string();
            let mut h = header;
            h.content_hash = fnv1a64(body.as_bytes());
            let ok = write_atomic_with_fs(&*fs, &path, &serialize(&h, &body)).is_ok();
            JobResult {
                buffer_id,
                class: ResultClass::Durability,
                version,
                kind: JobKind::SwapWrite,
                merge: Box::new(move |editor| {
                    // INVARIANT: route via by_id_mut(buffer_id) — NEVER active(); the merge must
                    // target the originating buffer even after a buffer switch (multi-buffer, Effort 6).
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        b.swap_in_flight = false;
                        // Path-aware latch (Codex pre-merge): only claim "this version is on disk"
                        // if the file we wrote (`path`) is STILL this buffer's current swap file. A
                        // SaveAs that rekeyed the buffer's path while this write was in flight makes
                        // `path` stale (written under the old key) — latching it would wrongly
                        // suppress a fresh swap at the new path. On a mismatch, skip the latch so the
                        // new path recheckpoints on the next idle tick. We deliberately do NOT delete
                        // the stale file: the workspace permits the same path open in multiple
                        // buffers, so a co-open buffer may legitimately own this `swap_path` (Codex).
                        // Leaving one stale swap is harmless (at worst a misleading recovery prompt
                        // for the old path); deleting a live buffer's swap would be data loss.
                        if ok && swap_path(b.document.path.as_deref()).ok().as_ref() == Some(&path) {
                            b.last_swap_at = Some(ts);
                            b.swapped_version = Some(version);
                        }
                    }
                    if !ok {
                        editor.set_status_full(crate::status::StatusKind::Error, "swap write failed".to_string(),
                            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                    } // status global
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

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "wc-scratch-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ))
    }

    #[test]
    fn fnv_is_stable_and_distinguishes() {
        assert_eq!(fnv1a64(b"abc"), fnv1a64(b"abc"));
        assert_ne!(fnv1a64(b"/a/notes.md"), fnv1a64(b"/b/notes.md"));
    }

    /// Regression — idle 100% CPU spin. The main loop wakes on the swap deadline; basing
    /// that deadline on `last_edit_at` alone (a monotonic, never-cleared timestamp) returns a
    /// permanently past-due instant once any edit has happened, so `recv_timeout` got a 0-length
    /// timeout forever and the loop busy-spun. The real "swap work pending" signal is
    /// `pending()`: unsaved content whose version is not yet in the swap file. The loop and the
    /// Tick dispatch gate on it, so a settled or clean buffer schedules NO wake and the loop blocks.
    #[test]
    fn settled_buffer_is_not_pending_so_the_loop_can_block() {
        let now = 100 * T_MAX_MS; // long past any idle/max deadline

        // The raw deadline IS permanently past-due once last_edit_at is set — the spin source
        // the gate must suppress:
        assert_eq!(next_deadline_ms(now, Some(0), None), Some(now),
            "raw swap deadline is past-due forever off a monotonic last_edit_at");

        // (a) clean buffer (e.g. just saved) with a stale last_edit_at → no swap work.
        assert!(!pending(false, 7, Some(3)), "clean buffer → nothing to swap");
        // (b) dirty but the current version is already in the swap file → no swap work.
        assert!(!pending(true, 7, Some(7)), "current version already swapped → nothing to swap");
        // (c) genuinely pending: edited to a new version not yet swapped → wake is scheduled.
        assert!(pending(true, 8, Some(7)), "unsaved new version → pending");
        assert!(pending(true, 1, None), "first edit, never swapped → pending");
        assert!(next_deadline_ms(now, Some(now), None).is_some(),
            "pending work still schedules a wake so the swap actually gets written");
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
        assert!(matches!(assess(&crate::fsx::RealFs, Some(&p), Some(b"abc\n")), RecoveryDecision::OpenNormally));
    }

    #[test]
    fn recovery_hash_equal_discards_silently() {
        let p = std::env::temp_dir().join(format!("wc-eq-{}.md", std::process::id()));
        let body = "same\n";
        let h = SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(body.as_bytes()), version: 1, ts_ms: 1, pid: 1 };
        write_atomic(&swap_path(Some(&p)).unwrap(), &serialize(&h, body)).unwrap();
        // F on disk == swap body → swap adds nothing.
        assert!(matches!(assess(&crate::fsx::RealFs, Some(&p), Some(body.as_bytes())), RecoveryDecision::DiscardSilently));
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
        match assess(&crate::fsx::RealFs, Some(&p), Some(b"file version\n")) {
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

        let result = find_orphan_scratch_swap_in(&crate::fsx::RealFs, &dir);
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
    fn assess_over_cap_swap_opens_normally() {
        // Write an oversized swap at the doc's swap path; recovery must NOT slurp → OpenNormally.
        let p = scratch(); // a doc path
        let sp = swap_path(Some(&p)).unwrap();
        std::fs::write(&sp, "x".repeat(crate::limits::MAX_OPEN_BYTES as usize + 1)).unwrap();
        assert!(matches!(assess(&crate::fsx::RealFs, Some(&p), None), RecoveryDecision::OpenNormally));
        let _ = std::fs::remove_file(&sp);
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
        e.active_mut().document.version = 3;
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
          dispatch_swap_write(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(e.active().last_swap_at, Some(123), "merge records last_swap_at");
        let sp = swap_path(Some(&doc_path)).unwrap();
        let (h, body) = parse(&std::fs::read_to_string(&sp).unwrap()).unwrap();
        assert_eq!(body, "swap me\n");
        assert_eq!(h.version, 3);
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }

    /// The swap worker must write through `ctx.fs`, not a hardcoded `RealFs` — an injected
    /// fault at `Ctx` construction must be able to fail the write and, in turn, suppress the
    /// latch. Before this task the worker called `write_atomic` (always `RealFs` internally),
    /// so an injected `FaultFs` was unreachable from here.
    ///
    /// FAIL-VERIFY: change the worker's `write_atomic_with_fs(&*fs, ...)` back to plain
    /// `write_atomic(...)`, watch this fail (the real write succeeds and the latch sets
    /// regardless of the injected fault), then revert.
    #[test]
    fn dispatch_swap_write_uses_the_injected_fs_and_a_failed_write_does_not_latch() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use crate::test_support::{FaultAt, FaultFs};
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 456 } }

        let doc_path = std::env::temp_dir().join(format!(
            "wc-dispatch-swap-fault-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let mut e = Editor::new_from_text("swap me\n", Some(doc_path.clone()), (80, 24));
        e.active_mut().document.version = 3;
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let faulty: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(FaultFs::new(FaultAt::Create));
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: faulty };
          dispatch_swap_write(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.active().swapped_version.is_none(),
            "an injected write failure must NOT latch swapped_version");
        assert!(e.active().last_swap_at.is_none(),
            "an injected write failure must NOT record last_swap_at");
        let _ = std::fs::remove_file(swap_path(Some(&doc_path)).unwrap());
        let _ = std::fs::remove_file(&doc_path);
    }

    /// SaveAs-in-flight stale-path race (Codex pre-merge): a SwapWrite dispatched under the OLD
    /// path must NOT set the latch once the buffer has been rekeyed to a new path — otherwise
    /// `pending()` would read "already swapped" and suppress a fresh swap at the NEW path, leaving
    /// unsaved content with no recovery file. The stale file under the old path is left in place
    /// (a co-open buffer could own it) and cleaned up by this test.
    #[test]
    fn stale_path_swap_does_not_relatch_after_rekey() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 77 } }

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let old_path = std::env::temp_dir().join(format!("wc-rekey-old-{}-{}.md", std::process::id(), seq));
        let new_path = std::env::temp_dir().join(format!("wc-rekey-new-{}-{}.md", std::process::id(), seq));
        let mut e = Editor::new_from_text("unsaved body\n", Some(old_path.clone()), (80, 24));
        e.active_mut().document.version = 5; // dirty, edited
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        // Dispatch a swap — it captures the OLD path.
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
          dispatch_swap_write(&mut ctx); }
        // Simulate a SaveAs rekey landing before this swap's merge drains: the buffer's path is now
        // the NEW file, and the save-side clear reset the latch.
        e.active_mut().document.path = Some(new_path.clone());
        e.active_mut().swapped_version = None;
        // Now the stale (old-path) swap outcome merges.
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert!(e.active().swapped_version.is_none(),
            "a swap written under the OLD path must NOT relatch after the buffer was rekeyed");
        // The stale swap under the old path is intentionally NOT deleted (a co-open buffer could
        // legitimately own that swap_path); clean it up here in the test.
        let _ = std::fs::remove_file(swap_path(Some(&old_path)).unwrap());
        let _ = std::fs::remove_file(swap_path(Some(&new_path)).unwrap());
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&new_path);
    }

    #[test]
    fn panicked_swap_clears_in_flight() {
        let p = scratch(); std::fs::write(&p, "x\n").unwrap();
        let mut e = Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
        let id = e.active().id;
        e.active_mut().swap_in_flight = true;
        crate::jobs_apply::apply_outcome(
            crate::jobs::JobOutcome::Panicked {
                buffer_id: id, version: 1, kind: crate::jobs::JobKind::SwapWrite, msg: "boom".into() },
            &mut e);
        assert!(!e.active().swap_in_flight, "panicked swap must clear swap_in_flight");
        let _ = std::fs::remove_file(&p);
    }

    // ── H5: recovery-file cleanup enumerator (SAFETY-CRITICAL — no data loss) ──────────

    const DEAD_PID: u32 = 999_999; // /proc/999999 does not exist (asserted below)

    /// Create a doc file with `saved` bytes on disk, plus its CANONICAL swap file whose body is
    /// `swap_body` (header hash = fnv1a64(swap_body), matching build_header). `pid` sets the
    /// writing pid. Returns (doc_path, swap_path). When `saved == swap_body` the swap adds
    /// nothing (assess → DiscardSilently); when they differ it is recoverable (assess → Prompt).
    fn make_doc_with_swap(saved: &str, swap_body: &str, pid: u32) -> (std::path::PathBuf, std::path::PathBuf) {
        let p = scratch();
        std::fs::write(&p, saved).unwrap();
        let real = std::fs::canonicalize(&p).unwrap();
        let h = SwapHeader {
            realpath: Some(real.to_string_lossy().into_owned()),
            load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(swap_body.as_bytes()),
            version: 1, ts_ms: 1, pid,
        };
        let sp = swap_path(Some(&p)).unwrap();
        write_atomic(&sp, &serialize(&h, swap_body)).unwrap();
        (p, sp)
    }

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "wc-h5-{}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed), label));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The oracle's include verdict: a swap is cleanable ONLY when its body matches the saved
    /// file (DiscardSilently) AND the pid is dead. A diverged swap (Prompt) holds unsaved work
    /// and must NEVER be swept — the core no-data-loss guarantee.
    #[test]
    fn swap_is_cleanable_only_for_valueless_dead_pid_swaps() {
        #[cfg(target_os = "linux")]
        assert!(!pid_is_live(DEAD_PID), "test invariant: pid 999999 must not be live");

        // (a) DiscardSilently + dead pid → cleanable.
        let (p, sp) = make_doc_with_swap("saved\n", "saved\n", DEAD_PID);
        assert!(swap_is_cleanable(&crate::fsx::RealFs, &sp), "swap body == saved file → zero recovery value → cleanable");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);

        // (b) Prompt (diverged) → NEVER cleanable (recoverable unsaved content).
        let (p, sp) = make_doc_with_swap("on disk\n", "UNSAVED WORK\n", DEAD_PID);
        assert!(!swap_is_cleanable(&crate::fsx::RealFs, &sp), "a diverged, recoverable swap must never be cleanable — no data loss");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);

        // (c) live pid → excluded even though the body matches (a live writer owns it).
        let (p, sp) = make_doc_with_swap("saved\n", "saved\n", std::process::id());
        assert!(!swap_is_cleanable(&crate::fsx::RealFs, &sp), "a live-pid swap is never swept");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);
    }

    /// A valueless swap RELOCATED away from its canonical `swap_path(realpath)` is excluded:
    /// binding the verdict to the candidate stops a clean verdict for one file greenlighting the
    /// deletion of a DIFFERENT (recoverable) swap.
    #[test]
    fn swap_is_cleanable_excludes_relocated_and_realpath_none() {
        // Relocated: identical valueless swap content, but stored at a NON-canonical path.
        let (p, sp) = make_doc_with_swap("saved\n", "saved\n", DEAD_PID);
        let raw = std::fs::read_to_string(&sp).unwrap();
        let relocated = unique_dir("reloc").join("relocated.swp");
        std::fs::write(&relocated, &raw).unwrap();
        assert!(!swap_is_cleanable(&crate::fsx::RealFs, &relocated),
            "a swap not at its canonical swap_path(realpath) is excluded (stale/relocated twin)");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&relocated);

        // realpath = None → no doc to compare against → excluded (fail closed).
        let h = SwapHeader { realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"x\n"), version: 1, ts_ms: 1, pid: DEAD_PID };
        let none_swap = unique_dir("none").join("nopath.swp");
        write_atomic(&none_swap, &serialize(&h, "x\n")).unwrap();
        assert!(!swap_is_cleanable(&crate::fsx::RealFs, &none_swap), "a swap with no recorded realpath is excluded");
        let _ = std::fs::remove_file(&none_swap);
    }

    /// End-to-end through the real state dir: the scan + oracle include a DiscardSilently swap and
    /// exclude a Prompt swap. Membership-based (the shared state dir carries litter).
    #[test]
    fn enumerator_scan_includes_discard_silently_excludes_prompt() {
        let dir = state_dir().unwrap();
        let (p_ok, sp_ok)   = make_doc_with_swap("same\n", "same\n", DEAD_PID);        // DiscardSilently
        let (p_bad, sp_bad) = make_doc_with_swap("file\n", "swap unsaved\n", DEAD_PID); // Prompt
        let out = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &HashSet::new());
        assert!(out.contains(&sp_ok), "valueless swap is enumerated as cleanable");
        assert!(!out.contains(&sp_bad), "recoverable (Prompt) swap is NEVER enumerated — no data loss");
        for f in [&sp_ok, &sp_bad, &p_ok, &p_bad] { let _ = std::fs::remove_file(f); }
    }

    /// recovered-*.md dumps are included; unrelated files are ignored; a protected path (open
    /// buffer / session swap) is never offered even when it would otherwise qualify.
    #[test]
    fn enumerator_includes_recovered_dumps_honors_protected() {
        let dir = unique_dir("recovered");
        let dump = dir.join("recovered-notes.md-123-0.md");
        std::fs::write(&dump, "extracted\n").unwrap();
        let other = dir.join("notes.md");            // not a recovered dump, not a swap → ignored
        std::fs::write(&other, "x\n").unwrap();
        let protected_dump = dir.join("recovered-open.md-123-1.md");
        std::fs::write(&protected_dump, "y\n").unwrap();

        let mut protected = HashSet::new();
        protected.insert(protected_dump.clone());
        let out = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &protected);
        assert!(out.contains(&dump), "a recovered-*.md dump is cleanable");
        assert!(!out.contains(&other), "an unrelated file is never touched");
        assert!(!out.contains(&protected_dump), "a protected path is never offered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `.tmp` atomic-write temps: include ONLY a byte-identical duplicate of an existing target;
    /// EXCLUDE any divergence, a missing target, or our own live-session temp.
    #[test]
    fn enumerator_tmp_only_byte_identical_duplicate_is_cleanable() {
        let dir = unique_dir("tmp");
        // (a) temp byte-identical to a committed target → valueless duplicate → included.
        let target = dir.join("doc.md-abcd.swp");
        std::fs::write(&target, "committed\n").unwrap();
        let tmp_same = dir.join(format!(".doc.md-abcd.swp.wcartel-{DEAD_PID}-0.tmp"));
        std::fs::write(&tmp_same, "committed\n").unwrap();
        // (b) temp DIVERGING from its target → may be the newest snapshot → excluded.
        let target2 = dir.join("doc2.md-ef01.swp");
        std::fs::write(&target2, "old\n").unwrap();
        let tmp_diff = dir.join(format!(".doc2.md-ef01.swp.wcartel-{DEAD_PID}-1.tmp"));
        std::fs::write(&tmp_diff, "NEWER UNSAVED\n").unwrap();
        // (c) temp whose target is MISSING → may be the only copy → excluded.
        let tmp_orphan = dir.join(format!(".gone.md-9999.swp.wcartel-{DEAD_PID}-2.tmp"));
        std::fs::write(&tmp_orphan, "only copy\n").unwrap();
        // (d) our OWN live-session temp, even byte-identical → never swept.
        let tmp_self = dir.join(format!(".doc.md-abcd.swp.wcartel-{}-3.tmp", std::process::id()));
        std::fs::write(&tmp_self, "committed\n").unwrap();

        let out = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &HashSet::new());
        assert!(out.contains(&tmp_same), "byte-identical temp duplicate is valueless → cleanable");
        assert!(!out.contains(&tmp_diff), "a temp diverging from its target may hold newer work → excluded");
        assert!(!out.contains(&tmp_orphan), "a temp whose target is missing may be the only copy → excluded");
        assert!(!out.contains(&tmp_self), "our own live session's temp is never swept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_swap_paths_covers_open_buffers_and_session_scratch() {
        let p = scratch();
        let e = Editor::new_from_text("hi\n", Some(p.clone()), (80, 24));
        let set = open_swap_paths(&e);
        assert!(set.contains(&swap_path(Some(&p)).unwrap()), "the open buffer's swap is protected");
        assert!(set.contains(&swap_path(None).unwrap()), "this session's scratch swap is protected");
    }

    #[test]
    fn enumerator_empty_dir_yields_nothing() {
        let dir = unique_dir("empty");
        assert!(cleanable_recovery_files(&crate::fsx::RealFs, &dir, &HashSet::new()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
