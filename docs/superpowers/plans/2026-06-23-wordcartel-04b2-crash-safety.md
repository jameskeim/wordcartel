# Wordcartel Effort 4b-2 — Crash Safety: Swap/Recovery, Panic Dump, External-Mod & Modals — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Prerequisite:** Effort **4b-1** (`2026-06-23-wordcartel-04b1-async-substrate.md`) is merged. This plan uses its types verbatim: `jobs::{Job, JobResult, JobKind::SwapWrite, Executor}`, `registry::Ctx`, `app::{Msg, reduce, apply_result}`, `save::{FileFingerprint, fingerprint}`, and `Document::{version, saved_version, dirty(), stored_fp, path}`.

**Goal:** Make the parent spec's "never lose the user's work" guarantee real: a periodic swap/recovery file that never overwrites the `.md`, a panic-time buffer dump, external-modification detection with a real prompt, and the shared modal mechanism that serves quit-confirm, external-mod, and recovery.

**Architecture:** A `swap` module writes an atomic, `0600`, header+body snapshot into the `0700` XDG state dir on a `JobKind::SwapWrite` job, driven by an idle-debounce + max-interval timer riding the unified loop's `Tick`. A `prompt` module provides a generic modal (`Prompt` + `PromptAction`) that intercepts input; `app::reduce` routes a chosen key to a resolver. Recovery-on-open compares a content hash (primary) and load-time stat (tiebreaker). A `recovery` module records a process-global last-good snapshot updated after each `apply`; the panic hook `try_lock`s it and writes a best-effort dump.

**Tech Stack:** Rust 2021, `ropey` 1.6.1 (LF/CRLF-only per 4b-1), `crossterm` 0.28, `dirs` 5 (XDG state dir), `std::thread`/`mpsc`, inline FNV-1a (no hash dep), `proptest` (dev).

## Global Constraints

(Copied from spec §3; bind every task.)

- **Responsiveness #1** (§3.9): foreground never blocks on IO; swap writes go through a job, never inline. `status` set before dispatch.
- **Functional core / imperative shell** (§10): all IO/threads/OS calls in the `wordcartel` shell crate; `wordcartel-core` untouched.
- **Never lose work / never crash silently** (§15.1–15.2): the `.md` is never written by the swap path; failures surface non-blocking; modal only for genuinely destructive/ambiguous decisions.
- **Single mutation channel** (§10.1): document text changes only via `editor.apply`; the `SwapWrite` merge is status-only bookkeeping.
- **LF-only line semantics** (from 4b-1).
- **Permissions (Unix):** state dir `0700`; every swap/temp/final/panic-dump file `0600`, mode set at temp-create time before rename.
- **Workspace facts:** `cargo test` from repo root; 4b-1 baseline green. No test weakened to pass.

---

## File Structure

- `wordcartel/src/prompt.rs` *(new)* — `Prompt`, `Choice`, `PromptAction`; lookup + render data. Generic modal mechanism (§5.3).
- `wordcartel/src/swap.rs` *(new)* — state-dir resolution, swap path, FNV hash, header (de)serialization, atomic `0600` write, cadence, recovery scan/predicate, lifecycle delete.
- `wordcartel/src/recovery.rs` *(new)* — process-global last-good snapshot + `write_dump` routine (§5.5).
- `wordcartel/src/editor.rs` *(modify)* — add `prompt: Option<Prompt>`, `last_edit_at`/`last_swap_at: Option<u64>`, `quit_after_save: Option<u64>`; record last-good snapshot in `apply`.
- `wordcartel/src/save.rs` *(modify)* — `dispatch_save` raises the external-mod modal instead of a status refusal; add `overwrite_save`; refresh fingerprint already present.
- `wordcartel/src/app.rs` *(modify)* — `reduce` intercepts active prompts; `Tick` drives swap cadence; `run` uses `recv_timeout`; bounded save&quit; recovery prompt on open; install the panic dump.
- `wordcartel/src/term.rs` *(modify)* — extend the panic hook to dump the last-good snapshot (`try_lock`).
- `wordcartel/src/lib.rs` *(modify)* — declare `prompt`, `swap`, `recovery`.
- `wordcartel/Cargo.toml` *(modify)* — add `dirs = "5"`.

---

## Task 1: modal-prompt infrastructure (spec §5.3)

**A generic single-line modal** that any of the three destructive decisions reuses. The mechanism is pure data — a message plus labeled choices, each carrying a `PromptAction` the resolver interprets. Wiring the first real consumer (quit) is Task 8; here we build and unit-test the mechanism in isolation.

**Files:**
- Create: `wordcartel/src/prompt.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod prompt;`)
- Modify: `wordcartel/src/editor.rs` (add `pub prompt: Option<crate::prompt::Prompt>,` to `Editor`, init `None`)
- Test: `wordcartel/src/prompt.rs`

**Interfaces:**
- Produces:
  - `pub enum PromptAction { Cancel, QuitAnyway, SaveAndQuit, Reload, Overwrite, Recover, DiscardSwap, OpenOriginal }` (derives `Clone, Copy, PartialEq, Eq, Debug`)
  - `pub struct Choice { pub key: char, pub label: &'static str, pub action: PromptAction }`
  - `pub struct Prompt { pub message: String, pub choices: Vec<Choice> }`
  - `impl Prompt { pub fn action_for(&self, ch: char) -> Option<PromptAction> }` — case-insensitive match on `key`.
  - Constructors: `Prompt::quit_confirm()`, `Prompt::external_mod()`, `Prompt::swap_recovery()`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/prompt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quit_confirm_routes_keys_case_insensitively() {
        let p = Prompt::quit_confirm();
        assert_eq!(p.action_for('s'), Some(PromptAction::SaveAndQuit));
        assert_eq!(p.action_for('Q'), Some(PromptAction::QuitAnyway));
        assert_eq!(p.action_for('c'), Some(PromptAction::Cancel));
        assert_eq!(p.action_for('z'), None, "unmapped key returns None");
    }
    #[test]
    fn external_mod_offers_reload_overwrite_and_disabled_saveas() {
        let p = Prompt::external_mod();
        assert_eq!(p.action_for('r'), Some(PromptAction::Reload));
        assert_eq!(p.action_for('o'), Some(PromptAction::Overwrite));
        // Save-as is deferred to Effort 5: not an actionable choice in 4b.
        assert_eq!(p.action_for('s'), None);
        assert!(p.message.to_lowercase().contains("changed on disk"));
    }
    #[test]
    fn swap_recovery_offers_recover_discard_open() {
        let p = Prompt::swap_recovery();
        assert_eq!(p.action_for('r'), Some(PromptAction::Recover));
        assert_eq!(p.action_for('d'), Some(PromptAction::DiscardSwap));
        assert_eq!(p.action_for('o'), Some(PromptAction::OpenOriginal));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib prompt::tests`
Expected: FAIL — module `prompt` does not exist.

- [ ] **Step 3: Declare the module** in `wordcartel/src/lib.rs`: `pub mod prompt;` (after `pub mod term;`).

- [ ] **Step 4: Write `wordcartel/src/prompt.rs`:**

```rust
//! Generic single-line modal (spec §5.3). Reserved for destructive/ambiguous
//! decisions: quit-with-unsaved, external modification, swap recovery. Pure
//! data; the resolver (app.rs) interprets the chosen PromptAction.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptAction {
    Cancel,
    QuitAnyway,
    SaveAndQuit,
    Reload,
    Overwrite,
    Recover,
    DiscardSwap,
    OpenOriginal,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub key: char,
    pub label: &'static str,
    pub action: PromptAction,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub message: String,
    pub choices: Vec<Choice>,
}

impl Prompt {
    /// Map a typed key to its action (case-insensitive on the choice key).
    pub fn action_for(&self, ch: char) -> Option<PromptAction> {
        let lc = ch.to_ascii_lowercase();
        self.choices
            .iter()
            .find(|c| c.key.to_ascii_lowercase() == lc)
            .map(|c| c.action)
    }

    pub fn quit_confirm() -> Prompt {
        Prompt {
            message: "Unsaved changes: [S]ave & quit · [Q]uit anyway · [C]ancel".into(),
            choices: vec![
                Choice { key: 's', label: "Save & quit", action: PromptAction::SaveAndQuit },
                Choice { key: 'q', label: "Quit anyway", action: PromptAction::QuitAnyway },
                Choice { key: 'c', label: "Cancel",      action: PromptAction::Cancel },
            ],
        }
    }

    pub fn external_mod() -> Prompt {
        Prompt {
            // Save-as ([S]) is deferred to Effort 5 — omitted from the choices.
            message: "File changed on disk: [R]eload · [O]verwrite  (Save-as: Effort 5)".into(),
            choices: vec![
                Choice { key: 'r', label: "Reload",    action: PromptAction::Reload },
                Choice { key: 'o', label: "Overwrite", action: PromptAction::Overwrite },
            ],
        }
    }

    pub fn swap_recovery() -> Prompt {
        Prompt {
            message: "Recovery file found: [R]ecover · [D]iscard · [O]pen original".into(),
            choices: vec![
                Choice { key: 'r', label: "Recover",       action: PromptAction::Recover },
                Choice { key: 'd', label: "Discard swap",  action: PromptAction::DiscardSwap },
                Choice { key: 'o', label: "Open original", action: PromptAction::OpenOriginal },
            ],
        }
    }
}
```

- [ ] **Step 5: Add the `prompt` field to `Editor`** in `wordcartel/src/editor.rs`: add `pub prompt: Option<crate::prompt::Prompt>,` to the struct and `prompt: None,` to `new_from_text`.

- [ ] **Step 6: Run tests + full suite**

Run: `cargo test -p wordcartel --lib prompt:: && cargo test`
Expected: PASS — 3 new tests; nothing else regresses (the new field is inert).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/prompt.rs wordcartel/src/editor.rs wordcartel/src/lib.rs
git commit -m "feat(prompt): generic modal mechanism (quit/external-mod/recovery actions)"
```

---

## Task 2: state-dir, swap path & permissions (spec §5.1)

**Resolve the `0700` XDG state dir and compute the per-document swap path** (`<sanitized-name>-<fnvhash(realpath)>.swp`, or `scratch-<pid>.swp`). Add `dirs`. Provide the FNV-1a hash used both here and by the recovery predicate (Task 3).

**Files:**
- Create: `wordcartel/src/swap.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod swap;`)
- Modify: `wordcartel/Cargo.toml` (add `dirs = "5"`)
- Test: `wordcartel/src/swap.rs`

**Interfaces:**
- Produces:
  - `pub fn fnv1a64(bytes: &[u8]) -> u64` — stable, dependency-free content hash.
  - `pub fn state_dir() -> std::io::Result<std::path::PathBuf>` — creates `$XDG_STATE_HOME/wordcartel` (`0700` on Unix), fallback `~/.local/state/wordcartel`.
  - `pub fn swap_path(doc_path: Option<&std::path::Path>) -> std::io::Result<std::path::PathBuf>` — named → `<sanitized>-<hexhash>.swp`; scratch → `scratch-<pid>.swp`, both inside `state_dir()`.
  - `pub fn sanitize(name: &str) -> String` — strip path separators / non-`[A-Za-z0-9._-]` to `_`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/swap.rs`:

```rust
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib swap::tests`
Expected: FAIL — module `swap` / `dirs` missing.

- [ ] **Step 3: Add the dependency** to `wordcartel/Cargo.toml` under `[dependencies]`: `dirs = "5"`.

- [ ] **Step 4: Declare the module** in `wordcartel/src/lib.rs`: `pub mod swap;`.

- [ ] **Step 5: Write the path/hash core of `wordcartel/src/swap.rs`:**

```rust
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
```

- [ ] **Step 6: Run tests + full suite**

Run: `cargo test -p wordcartel --lib swap::tests && cargo test`
Expected: PASS — 5 swap tests (4 on non-Unix); nothing regresses.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/swap.rs wordcartel/src/lib.rs wordcartel/Cargo.toml Cargo.lock
git commit -m "feat(swap): state-dir (0700), swap-path, FNV-1a content hash"
```

---

## Task 3: swap header (de)serialization + content hash (spec §5.1)

**The swap file is a small text header followed by the full buffer text.** Header fields: format version, realpath (or `-` for scratch), load-time `(mtime, size)` (or `-` if F did not exist at load), the buffer content hash (recovery predicate's primary key), document version, timestamp, pid.

**Files:**
- Modify: `wordcartel/src/swap.rs` (add `SwapHeader`, `serialize`, `parse`)
- Test: `wordcartel/src/swap.rs`

**Interfaces:**
- Produces:
  - `pub struct SwapHeader { pub realpath: Option<String>, pub load_mtime_secs: Option<u64>, pub load_size: Option<u64>, pub content_hash: u64, pub version: u64, pub ts_ms: u64, pub pid: u32 }` (derives `Clone, PartialEq, Eq, Debug`)
  - `pub fn serialize(header: &SwapHeader, body: &str) -> String`
  - `pub fn parse(text: &str) -> Option<(SwapHeader, String)>` — `None` on malformed/unknown-format input (caller treats as "prompt" conservatively).

- [ ] **Step 1: Write the failing test** in `wordcartel/src/swap.rs` tests:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel --lib swap::tests::header_round_trips`
Expected: FAIL — `SwapHeader`/`serialize`/`parse` do not exist.

- [ ] **Step 3: Add header (de)serialization** to `wordcartel/src/swap.rs`:

```rust
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
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p wordcartel --lib swap::tests`
Expected: PASS — round-trip (named + scratch) and reject-unknown.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/swap.rs
git commit -m "feat(swap): header serialize/parse with content hash + Option fp fields"
```

---

## Task 4: atomic swap write as a SwapWrite job (spec §5.1, §5.2)

**Write the swap atomically, `0600`, on a `JobKind::SwapWrite` job** — the first non-save substrate consumer, validating its generality. The merge is status-only bookkeeping.

**Files:**
- Modify: `wordcartel/src/swap.rs` (add `write_atomic`, `build_header`, `dispatch_swap_write`)
- Test: `wordcartel/src/swap.rs`

**Interfaces:**
- Consumes: `jobs::{Job, JobResult, JobKind, Executor}`, `registry::Ctx`, `swap::{serialize, SwapHeader, fnv1a64, swap_path}`.
- Produces:
  - `pub fn write_atomic(path: &Path, content: &str) -> io::Result<()>` — same-dir O_EXCL temp (`0600`), write, fsync, rename. No symlink/skip-unchanged logic (our own files).
  - `pub fn build_header(editor: &Editor, body: &str) -> SwapHeader` — fills `content_hash = fnv1a64(body.as_bytes())`, `version`, realpath, load fp from `editor`, `ts_ms` from `clock`, `pid`.
  - `pub fn dispatch_swap_write(ctx: &mut Ctx)` — capture O(1) snapshot + header inputs, dispatch a `SwapWrite` job; merge sets `editor.last_swap_at`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/swap.rs` tests:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib swap::tests::write_atomic_writes_0600_and_roundtrips_via_parse swap::tests::dispatch_swap_write_writes_a_recoverable_swap`
Expected: FAIL — `write_atomic`/`dispatch_swap_write`/`last_swap_at` missing.

- [ ] **Step 3: Add `last_swap_at`/`last_edit_at` to `Editor`** in `wordcartel/src/editor.rs`: add `pub last_edit_at: Option<u64>,` and `pub last_swap_at: Option<u64>,` to the struct; init both `None` in `new_from_text`.

- [ ] **Step 4: Add the atomic write + dispatch** to `wordcartel/src/swap.rs`:

```rust
use crate::editor::Editor;
use crate::jobs::{Job, JobKind, JobResult};
use crate::registry::Ctx;
use std::io::Write as _;

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
```

- [ ] **Step 5: Run tests + full suite**

Run: `cargo test -p wordcartel --lib swap::tests && cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/swap.rs wordcartel/src/editor.rs
git commit -m "feat(swap): atomic 0600 write + SwapWrite job dispatch"
```

---

## Task 5: swap cadence — idle-debounce + max-cap on the timer (spec §5.2)

**Drive swap writes from the unified loop's `Tick`.** Write `T_idle` (2 s) after the last edit, but force one at least every `T_max` (30 s) during continuous editing, only while dirty. The loop uses `recv_timeout` to the next deadline.

**Files:**
- Modify: `wordcartel/src/swap.rs` (add cadence predicate + constants)
- Modify: `wordcartel/src/app.rs` (`reduce` records `last_edit_at`; `Tick` checks cadence and dispatches; `run` uses `recv_timeout`)
- Test: `wordcartel/src/swap.rs` (pure cadence), `wordcartel/src/app.rs` (Tick integration via InlineExecutor)

**Interfaces:**
- Produces:
  - `pub const T_IDLE_MS: u64 = 2_000; pub const T_MAX_MS: u64 = 30_000;`
  - `pub fn due(now: u64, last_edit_at: Option<u64>, last_swap_at: Option<u64>) -> bool` — true iff there was an edit and either idle-elapsed since the last edit or max-elapsed since the last swap.
  - `pub fn next_deadline_ms(now: u64, last_edit_at: Option<u64>, last_swap_at: Option<u64>) -> Option<u64>` — earliest future instant the loop should wake to write (for `recv_timeout`), or `None` when nothing is pending.

- [ ] **Step 1: Write the failing cadence tests** in `wordcartel/src/swap.rs` tests:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib swap::tests::cadence`
Expected: FAIL — `due`/constants missing.

- [ ] **Step 3: Add the cadence predicate** to `wordcartel/src/swap.rs`:

```rust
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
    let idle_at = edit + T_IDLE_MS;
    let max_at = last_swap_at.unwrap_or(edit) + T_MAX_MS;
    Some(idle_at.min(max_at).max(now))
}
```

- [ ] **Step 4: Wire `reduce` to record edits and `Tick` to write.** In `wordcartel/src/app.rs` `reduce`, after handling an `Input` message, record `last_edit_at` when the version advanced, and on `Tick` dispatch a swap if due + dirty:

```rust
pub fn reduce(
    msg: Msg, editor: &mut Editor, reg: &Registry, ex: &dyn Executor, clock: &dyn Clock,
) -> bool {
    // (prompt interception added in Task 8 goes ABOVE this match)
    let before = editor.document.version;
    match msg {
        Msg::Input(Event::Key(key)) => { /* …unchanged dispatch from 4b-1… */ }
        Msg::Input(Event::Resize(w, h)) => { editor.view.area = (w, h); derive::rebuild(editor); }
        Msg::Input(_) => {}
        Msg::JobDone(r) => apply_result(r, editor),
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.document.dirty()
                && crate::swap::due(now, editor.last_edit_at, editor.last_swap_at)
            {
                let mut ctx = Ctx { editor, clock, executor: ex };
                crate::swap::dispatch_swap_write(&mut ctx);
                // Provisionally mark; the merge confirms with the same ts.
                ctx.editor.last_swap_at = Some(now);
            }
        }
    }
    if editor.document.version != before {
        editor.last_edit_at = Some(clock.now_ms());
    }
    for r in ex.drain() { apply_result(r, editor); }
    !editor.quit
}
```

- [ ] **Step 5: Make `run` wake on the swap deadline.** In `wordcartel/src/app.rs` `run`, replace the `for msg in msg_rx` loop with a `recv_timeout` loop that synthesizes `Tick` on timeout:

```rust
    guard.terminal().draw(|f| render::render(f, &editor))?;
    loop {
        let now = clock.now_ms();
        let timeout = crate::swap::next_deadline_ms(now, editor.last_edit_at, editor.last_swap_at)
            .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
            .unwrap_or(std::time::Duration::from_secs(3600)); // idle: effectively block
        let msg = match msg_rx.recv_timeout(timeout) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Msg::Tick,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let keep = reduce(msg, &mut editor, &reg, &executor, &clock);
        guard.terminal().draw(|f| render::render(f, &editor))?;
        if !keep { break; }
    }
    Ok(())
```

- [ ] **Step 6: Write the Tick integration test** in `wordcartel/src/app.rs` tests:

```rust
#[test]
fn tick_writes_swap_when_idle_elapsed_and_dirty() {
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use crate::registry::Registry;
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    e.document.version = 1;            // dirty (saved_version=Some(0))
    e.last_edit_at = Some(0);
    let reg = Registry::builtins();
    let ex = InlineExecutor::default();
    // Clock past the idle threshold.
    struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
    let clk = C(crate::swap::T_IDLE_MS + 5);
    crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &ex, &clk);
    assert!(e.last_swap_at.is_some(), "an idle Tick on a dirty buffer writes a swap");
    let sp = crate::swap::swap_path(None).unwrap();
    assert!(sp.exists());
    let _ = std::fs::remove_file(&sp);
}
```

- [ ] **Step 7: Run tests + full suite**

Run: `cargo test`
Expected: PASS — cadence unit tests + Tick integration + everything prior.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/swap.rs wordcartel/src/app.rs
git commit -m "feat(swap): idle-debounce + max-cap cadence on the unified-loop Tick"
```

---

## Task 6: swap lifecycle — version-aware delete on clean (spec §5.1)

**Delete the swap on clean quit and on a save that leaves the buffer clean** (`saved_version == Some(version)`), recreate on the next edit. A background save that completes for an older snapshot while the buffer is dirty must **keep** the swap.

**Files:**
- Modify: `wordcartel/src/swap.rs` (add `delete`)
- Modify: `wordcartel/src/save.rs` (save merge deletes the swap only when it leaves the buffer clean)
- Test: `wordcartel/src/save.rs`

**Interfaces:**
- Produces: `pub fn delete(doc_path: Option<&Path>)` — best-effort `remove_file(swap_path(..))`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/save.rs` tests:

```rust
    #[test]
    fn save_clean_deletes_swap_but_stale_save_keeps_it() {
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();

        // Pre-create a swap for this doc.
        let sp = crate::swap::swap_path(Some(&p)).unwrap();
        crate::swap::write_atomic(&sp, "stub").unwrap();
        assert!(sp.exists());

        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.document.saved_version = None;
        e.document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(!e.document.dirty());
        assert!(!sp.exists(), "a save that leaves the buffer clean deletes the swap");

        // Now: dispatch a save at v2, but edit on to v3 before the merge → keep swap.
        crate::swap::write_atomic(&sp, "stub2").unwrap();
        e.document.version = 2;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        e.document.version = 3; // edited on
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.document.dirty());
        assert!(sp.exists(), "a stale-version save must NOT delete the swap");
        let _ = std::fs::remove_file(&sp); let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel --lib save::tests::save_clean_deletes_swap_but_stale_save_keeps_it`
Expected: FAIL — `swap::delete` missing; save merge does not delete the swap.

- [ ] **Step 3: Add `delete`** to `wordcartel/src/swap.rs`:

```rust
/// Best-effort delete of a document's swap file.
pub fn delete(doc_path: Option<&Path>) {
    if let Ok(p) = swap_path(doc_path) {
        let _ = std::fs::remove_file(p);
    }
}
```

- [ ] **Step 4: Delete the swap in the save merge only when clean.** In `wordcartel/src/save.rs`, inside the `Ok(Saved | Unchanged)` arm of the merge, after setting `saved_version`/`stored_fp`, add the version-aware deletion:

```rust
                        Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                            editor.document.saved_version = Some(v);
                            editor.document.stored_fp = new_fp;
                            if editor.document.version == v {
                                editor.status = "Saved".to_string();
                                // Buffer is clean at the saved version → swap is
                                // no longer needed. (Stale-version saves skip this.)
                                crate::swap::delete(editor.document.path.as_deref());
                            } else {
                                editor.status = format!("Saved v{v} (still editing)");
                            }
                        }
```

- [ ] **Step 5: Run tests + full suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/swap.rs wordcartel/src/save.rs
git commit -m "feat(swap): version-aware lifecycle — delete on clean save, keep on stale"
```

---

## Task 7: recovery-on-open — hash-first predicate + modal (spec §5.1)

**On open, decide via content hash (primary) and stat (tiebreaker).** No swap → open normally. Swap hash == hash(F bytes) → silently discard, open F. Otherwise → raise the swap-recovery modal.

**Files:**
- Modify: `wordcartel/src/swap.rs` (add `RecoveryDecision`, `assess`)
- Modify: `wordcartel/src/app.rs` (`run` open path: assess + set `editor.prompt`; resolver handles Recover/Discard/OpenOriginal — added with Task 8's resolver)
- Test: `wordcartel/src/swap.rs`

**Interfaces:**
- Produces:
  - `pub enum RecoveryDecision { OpenNormally, DiscardSilently, Prompt(SwapHeader, String) }` (the `String` is the swap body for a possible Recover).
  - `pub fn assess(doc_path: Option<&Path>, current_file_bytes: Option<&[u8]>) -> RecoveryDecision` — pure given the file bytes (the caller reads them once). Scratch (`doc_path == None`) with a non-empty swap → `Prompt`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/swap.rs` tests:

```rust
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
```

(Add `#[derive(Debug)]` to `RecoveryDecision` so `panic!("{other:?}")` compiles.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib swap::tests::recovery`
Expected: FAIL — `assess`/`RecoveryDecision` missing.

- [ ] **Step 3: Add the predicate** to `wordcartel/src/swap.rs`:

```rust
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
```

- [ ] **Step 4: Wire the open path in `run`.** In `wordcartel/src/app.rs` `run`, after opening the file and before the loop, assess recovery and (on `Prompt`) stash the swap body + set the modal. Add fields to carry the pending swap body for the resolver. In `editor.rs` add `pub pending_swap_body: Option<String>,` (init `None`). In `run`:

```rust
    // Recovery-on-open (§5.1). Read F's current bytes once for the predicate.
    let file_bytes = editor.document.path.as_deref().and_then(|p| std::fs::read(p).ok());
    match crate::swap::assess(editor.document.path.as_deref(), file_bytes.as_deref()) {
        crate::swap::RecoveryDecision::OpenNormally => {}
        crate::swap::RecoveryDecision::DiscardSilently => {
            crate::swap::delete(editor.document.path.as_deref());
        }
        crate::swap::RecoveryDecision::Prompt(_h, body) => {
            editor.pending_swap_body = Some(body);
            editor.prompt = Some(crate::prompt::Prompt::swap_recovery());
            editor.status = "Recovery file found".into();
        }
    }
```

(The Recover/Discard/OpenOriginal resolver is implemented in Task 8 alongside the shared prompt routing; until then the prompt renders but the keys are inert — acceptable mid-plan, finalized next task.)

- [ ] **Step 5: Run tests + full suite**

Run: `cargo test`
Expected: PASS — recovery predicate tests; nothing regresses.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/swap.rs wordcartel/src/app.rs wordcartel/src/editor.rs
git commit -m "feat(swap): hash-first recovery predicate + recovery prompt on open"
```

---

## Task 8: prompt routing + quit modal + bounded save&quit (spec §5.3)

**Make active prompts intercept input and route a chosen key to a resolver.** Upgrade quit-with-unsaved from double-Ctrl+Q to the real modal. "Save & quit" dispatches the save and waits for **that save's version** with a 5 s timeout; on success delete the swap and exit; on error/timeout re-raise the prompt. Also resolve the recovery actions from Task 7.

**Files:**
- Modify: `wordcartel/src/app.rs` (prompt interception in `reduce`; `resolve_prompt`; quit handler raises the modal; save&quit bounded wait in `run`)
- Modify: `wordcartel/src/editor.rs` (add `pub quit_after_save: Option<u64>,` init `None`)
- Modify: `wordcartel/src/commands.rs` (the `Command::Quit` arm raises the modal instead of the double-press flag)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Produces:
  - `pub fn resolve_prompt(action: PromptAction, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock)` — performs the effect and clears `editor.prompt`. `SaveAndQuit` dispatches the save and sets `editor.quit_after_save = Some(version)`; `QuitAnyway` sets `editor.quit = true`; `Cancel` clears; `Reload`/`Overwrite` (Task 9); `Recover`/`DiscardSwap`/`OpenOriginal` act on `pending_swap_body`.
  - `apply_result` extended: a successful `Save` whose `version == quit_after_save` sets `editor.quit = true`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/app.rs` tests:

```rust
#[test]
fn quit_with_unsaved_raises_modal_then_quit_anyway_exits() {
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("x\n", None, (80, 24));
    e.document.version = 1; // dirty
    let reg = Registry::builtins();
    let ex = InlineExecutor::default();
    let clk = TestClock(0);
    let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press, state: KeyEventState::NONE });
    // First Ctrl+Q → modal up, not quit.
    crate::app::reduce(crate::app::Msg::Input(ctrl_q.clone()), &mut e, &reg, &ex, &clk);
    assert!(e.prompt.is_some() && !e.quit);
    // Press 'q' → routed to QuitAnyway.
    let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press, state: KeyEventState::NONE });
    crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &ex, &clk);
    assert!(e.quit, "Quit anyway exits");
    assert!(e.prompt.is_none(), "prompt cleared");
}

#[test]
fn save_and_quit_sets_quit_after_save_and_exits_on_matching_result() {
    use crate::editor::Editor;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::prompt::PromptAction;
    let p = std::env::temp_dir().join(format!("wc-savequit-{}.md", std::process::id()));
    std::fs::write(&p, "old\n").unwrap();
    let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
    e.document.saved_version = None; e.document.version = 1;
    let ex = InlineExecutor::default();
    let clk = TestClock(0);
    crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk);
    assert_eq!(e.quit_after_save, Some(1));
    assert!(!e.quit, "not yet — waiting for the save result");
    for r in ex.drain() { crate::app::apply_result(r, &mut e); }
    assert!(e.quit, "matching save result triggers quit");
    let _ = std::fs::remove_file(&p);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib app::tests::quit_with_unsaved_raises_modal_then_quit_anyway_exits app::tests::save_and_quit_sets_quit_after_save_and_exits_on_matching_result`
Expected: FAIL — prompt routing / `resolve_prompt` / `quit_after_save` missing.

- [ ] **Step 3: Add `quit_after_save`** to `Editor` (`editor.rs`), init `None`.

- [ ] **Step 4: Intercept prompts in `reduce`.** At the top of `reduce`, before the normal `match`, route input when a prompt is active:

```rust
    // Active modal intercepts all key input (§5.3).
    if editor.prompt.is_some() {
        if let Msg::Input(Event::Key(key)) = &msg {
            if key.kind == crossterm::event::KeyEventKind::Press {
                if let crossterm::event::KeyCode::Char(ch) = key.code {
                    if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                        resolve_prompt(action, editor, ex, clock);
                    }
                }
            }
        }
        // Esc cancels any prompt.
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.code == crossterm::event::KeyCode::Esc { editor.prompt = None; }
        }
        for r in ex.drain() { apply_result(r, editor); }
        return !editor.quit;
    }
```

- [ ] **Step 5: Implement `resolve_prompt`** in `wordcartel/src/app.rs`:

```rust
use crate::prompt::PromptAction;

pub fn resolve_prompt(action: PromptAction, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock) {
    match action {
        PromptAction::Cancel => {}
        PromptAction::QuitAnyway => { editor.quit = true; }
        PromptAction::SaveAndQuit => {
            let v = editor.document.version;
            let mut ctx = Ctx { editor, clock, executor: ex };
            crate::save::dispatch_save(&mut ctx);
            editor.quit_after_save = Some(v);
            editor.prompt = None;
            return; // keep prompt cleared; do not fall through
        }
        PromptAction::Reload => crate::save::reload_from_disk(editor),     // Task 9
        PromptAction::Overwrite => {                                       // Task 9
            let mut ctx = Ctx { editor, clock, executor: ex };
            crate::save::overwrite_save(&mut ctx);
        }
        PromptAction::Recover => {
            if let Some(body) = editor.pending_swap_body.take() {
                // Load the swap content into the buffer, mark dirty (saved_version
                // stays None), keep the original path.
                crate::save::load_recovered(editor, &body);
            }
        }
        PromptAction::DiscardSwap => crate::swap::delete(editor.document.path.as_deref()),
        PromptAction::OpenOriginal => { editor.pending_swap_body = None; }
    }
    editor.prompt = None;
}
```

- [ ] **Step 6: Extend `apply_result` for save&quit.** After merging a result, check the quit-after-save target:

```rust
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if is_stale(r.kind, r.version, editor.document.version) { return; }
    let (kind, version) = (r.kind, r.version);
    (r.merge)(editor);
    // Save & quit: exit once the awaited save version lands clean.
    if kind == crate::jobs::JobKind::Save
        && editor.quit_after_save == Some(version)
        && editor.document.saved_version == Some(version)
    {
        editor.quit = true;
    }
}
```

- [ ] **Step 7: Raise the modal from the quit command.** In `wordcartel/src/commands.rs`, change the `Command::Quit` arm to raise the modal on a dirty buffer (replacing the `pending_quit` double-press). A clean buffer still quits immediately:

```rust
        Command::Quit => {
            if editor.document.dirty() {
                editor.prompt = Some(crate::prompt::Prompt::quit_confirm());
                CommandResult::Handled
            } else {
                editor.quit = true;
                CommandResult::Quit
            }
        }
```

(The `pending_quit` field and its `step`-test become dead; update `app::tests::step_processes_typing_and_quit` to drive the modal: after the first Ctrl+Q assert `e.prompt.is_some()`, then feed `'q'` through `reduce` and assert `e.quit`. This preserves the test's intent — dirty-quit needs confirmation — through the new mechanism. Remove the now-unused `pending_quit` field and its clearing in `commands::run`.)

- [ ] **Step 8: Add the bounded save&quit wait to `run`.** In `wordcartel/src/app.rs` `run`, the `recv_timeout` already surfaces results promptly; add a deadline guard: when `editor.quit_after_save.is_some()`, bound the wait to 5 s and, on expiry, re-raise the prompt:

```rust
    const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;
    // …inside the loop, after computing `now`:
    if let Some(_v) = editor.quit_after_save {
        let waited = now.saturating_sub(editor.last_edit_at.unwrap_or(now));
        if waited > SAVE_QUIT_TIMEOUT_MS {
            editor.quit_after_save = None;
            editor.prompt = Some(crate::prompt::Prompt::quit_confirm());
            editor.status = "Save still running — choose again".into();
        }
    }
```

(The bounded-wait timeout is loop-level; the success path is covered by the `apply_result` unit test in Step 1.)

- [ ] **Step 9: Run tests + full suite**

Run: `cargo test`
Expected: PASS — quit modal + save&quit tests; the updated `step_processes_typing_and_quit`; everything prior.

- [ ] **Step 10: Manual smoke.** Edit, Ctrl+Q → modal; `c` cancels; Ctrl+Q then `s` saves & exits; Ctrl+Q then `q` exits dirty:

Run: `cargo run -p wordcartel -- /tmp/wcartel-smoke.md`

- [ ] **Step 11: Commit**

```bash
git add wordcartel/src/app.rs wordcartel/src/editor.rs wordcartel/src/commands.rs
git commit -m "feat(prompt): modal input routing; quit modal + bounded save&quit"
```

---

## Task 9: external-modification detection + prompt actions (spec §5.4)

**Upgrade 4b-1's status-refusal to the real modal,** define the `Option<FileFingerprint>` matrix (existed / new / scratch / deleted), and implement the Reload/Overwrite/recover-load resolver effects.

**Files:**
- Modify: `wordcartel/src/save.rs` (`dispatch_save` raises the modal; add `overwrite_save`, `reload_from_disk`, `load_recovered`)
- Test: `wordcartel/src/save.rs`

**Interfaces:**
- Produces:
  - `pub fn overwrite_save(ctx: &mut Ctx)` — re-runs the save flow **skipping** the fingerprint check.
  - `pub fn reload_from_disk(editor: &mut Editor)` — replace buffer text from disk, clear history, clamp caret, refresh `stored_fp`, mark clean (`saved_version = Some(version)` after a version bump), delete the swap.
  - `pub fn load_recovered(editor: &mut Editor, body: &str)` — replace buffer text with the swap body, mark dirty (`saved_version = None`), keep the path.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/save.rs` tests:

```rust
    #[test]
    fn dispatch_save_raises_modal_on_external_change() {
        use crate::jobs::InlineExecutor;
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "v0\n").unwrap();
        let mut e = Editor::new_from_text("mine\n", Some(p.clone()), (80, 24));
        // stored_fp captured at load == v0's fp. Now an external process rewrites F.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&p, "external change\n").unwrap();
        e.document.version = 1; e.document.saved_version = None;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "external change must raise the modal, not clobber");
        assert!(ex.drain().is_empty(), "no save job dispatched on conflict");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fingerprint_matrix_new_and_deleted_are_conflicts() {
        // New named buffer (stored_fp = None) but a file now exists → conflict.
        let p = scratch();
        std::fs::write(&p, "created externally\n").unwrap();
        let mut e = Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
        e.document.stored_fp = None;        // "did not exist at load"
        e.document.version = 1; e.document.saved_version = None;
        let ex = crate::jobs::InlineExecutor::default();
        let clk = Z;
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex };
          dispatch_save(&mut ctx); }
        assert!(e.prompt.is_some(), "a file appearing where there was none is a conflict");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn overwrite_save_bypasses_the_stat_check() {
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        let p = scratch();
        std::fs::write(&p, "v0\n").unwrap();
        let mut e = Editor::new_from_text("mine\n", Some(p.clone()), (80, 24));
        std::fs::write(&p, "external\n").unwrap(); // diverged
        e.document.version = 1; e.document.saved_version = None;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; overwrite_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine\n", "overwrite wins");
        assert!(!e.document.dirty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn reload_from_disk_resets_to_file_and_marks_clean() {
        let p = scratch();
        std::fs::write(&p, "on disk\n").unwrap();
        let mut e = Editor::new_from_text("in memory edits\n", Some(p.clone()), (80, 24));
        e.document.version = 4; e.document.saved_version = None;
        reload_from_disk(&mut e);
        assert_eq!(e.document.buffer.to_string(), "on disk\n");
        assert!(!e.document.dirty(), "reloaded buffer is clean");
        let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib save::tests::dispatch_save_raises_modal_on_external_change save::tests::overwrite_save_bypasses_the_stat_check save::tests::reload_from_disk_resets_to_file_and_marks_clean save::tests::fingerprint_matrix_new_and_deleted_are_conflicts`
Expected: FAIL — modal not raised; `overwrite_save`/`reload_from_disk` missing.

- [ ] **Step 3: Replace the status-refusal with the modal** in `dispatch_save` (`save.rs`). Change the external-mod block from setting a status to raising the prompt:

```rust
    let current_fp = fingerprint(&path);
    if current_fp != ctx.editor.document.stored_fp {
        // Existed/new/scratch/deleted all collapse to "stored_fp != on-disk now".
        ctx.editor.prompt = Some(crate::prompt::Prompt::external_mod());
        ctx.editor.status = "File changed on disk".to_string();
        return CommandResult::Handled;
    }
```

Then factor the dispatch body (status "Saving…" → snapshot → dispatch job) into a private `fn do_save(ctx: &mut Ctx)` so `overwrite_save` can reuse it without the stat check.

- [ ] **Step 4: Add `overwrite_save`, `reload_from_disk`, `load_recovered`** to `save.rs`:

```rust
/// Save bypassing the fingerprint conflict (the [O]verwrite modal action).
pub fn overwrite_save(ctx: &mut Ctx) {
    if ctx.editor.document.path.is_none() {
        ctx.editor.status = "No file name (save-as is Effort 5)".to_string();
        return;
    }
    do_save(ctx); // no stat check
}

/// [R]eload: discard in-memory edits, reload F from disk (destructive — only
/// reachable via the external-mod modal).
pub fn reload_from_disk(editor: &mut Editor) {
    let Some(path) = editor.document.path.clone() else { return };
    let text = match crate::file::open(&path) { Ok(t) => t, Err(e) => { editor.status = e.to_string(); return; } };
    let area = editor.view.area;
    let fresh = Editor::new_from_text(&text, Some(path.clone()), area);
    // Preserve the path; reset text/history/selection; refresh fp; mark clean.
    editor.document = fresh.document;
    editor.view.line_layouts.clear();
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.document.stored_fp = fingerprint(&path);
    editor.status = "Reloaded".into();
    crate::swap::delete(editor.document.path.as_deref());
}

/// Load recovered swap content into the buffer; keep the path; mark dirty.
pub fn load_recovered(editor: &mut Editor, body: &str) {
    let path = editor.document.path.clone();
    let area = editor.view.area;
    let fresh = Editor::new_from_text(body, path.clone(), area);
    editor.document = fresh.document;
    editor.document.saved_version = None; // recovered work is unsaved
    editor.view.line_layouts.clear();
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.document.stored_fp = path.as_deref().and_then(fingerprint);
    editor.status = "Recovered unsaved changes".into();
}
```

(`do_save` is the extracted body from Step 3 — the "Saving…" status, O(1) snapshot capture, and `executor.dispatch` of the `JobKind::Save` job with the version-aware merge from 4b-1 Task 9 / 4b-2 Task 6.)

- [ ] **Step 5: Run tests + full suite**

Run: `cargo test`
Expected: PASS — external-mod modal + matrix + overwrite + reload; everything prior.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/save.rs
git commit -m "feat(save): external-mod modal (Option<FileFingerprint> matrix) + reload/overwrite"
```

---

## Task 10: panic dump — global last-good snapshot + try_lock hook (spec §5.5)

**Record a process-global last-good snapshot after each `apply`; the panic hook `try_lock`s it and writes a best-effort `0600` dump.** A blocking `lock()` could deadlock if the panic fires mid-update, so the hook uses `try_lock`.

**Files:**
- Create: `wordcartel/src/recovery.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod recovery;`)
- Modify: `wordcartel/src/editor.rs` (`apply` records the snapshot)
- Modify: `wordcartel/src/term.rs` (panic hook dumps via `try_lock`)
- Test: `wordcartel/src/recovery.rs`

**Interfaces:**
- Produces:
  - `pub static LAST_GOOD: Mutex<Option<(Option<PathBuf>, ropey::Rope)>>` (or a `fn record_snapshot(path, rope)` wrapping it).
  - `pub fn record_snapshot(path: Option<&Path>, rope: ropey::Rope)` — overwrite the global last-good snapshot (O(1) rope clone).
  - `pub fn write_dump(path: Option<&Path>, rope: &ropey::Rope, dir: &Path) -> std::io::Result<PathBuf>` — write `recovered-<name>-<pid>.md` (`0600`) into `dir`; returns the written path. Tested directly with an injected snapshot (no real panic).
  - `pub fn dump_on_panic()` — `try_lock` the global; on success call `write_dump` into `swap::state_dir()`; best-effort (ignore errors / a held lock).

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/recovery.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib recovery::tests`
Expected: FAIL — module `recovery` does not exist.

- [ ] **Step 3: Declare the module** in `wordcartel/src/lib.rs`: `pub mod recovery;`.

- [ ] **Step 4: Write `wordcartel/src/recovery.rs`:**

```rust
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
```

- [ ] **Step 5: Record the snapshot in `apply`.** At the end of `Editor::apply` (`editor.rs`), after the version bump and derive-hints, add:

```rust
        crate::recovery::record_snapshot(self.document.path.as_deref(), self.document.buffer.snapshot());
```

- [ ] **Step 6: Extend the panic hook** in `wordcartel/src/term.rs` `install_panic_hook` to dump before chaining:

```rust
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort emergency dump (try_lock; never deadlock).
            crate::recovery::dump_on_panic();
            // Restore the terminal.
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
            prev(info);
        }));
```

- [ ] **Step 7: Run tests + full suite**

Run: `cargo test`
Expected: PASS — dump-routine tests (named + scratch); the `apply` snapshot record is inert to existing tests; everything prior green.

- [ ] **Step 8: Manual smoke (optional).** Temporarily add a `panic!()` behind a debug key, run, trigger it, confirm the terminal is restored and a `recovered-*.md` appears in the state dir; then remove the debug panic.

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/recovery.rs wordcartel/src/editor.rs wordcartel/src/term.rs wordcartel/src/lib.rs
git commit -m "feat(recovery): panic-time buffer dump (global snapshot + try_lock hook)"
```

---

## Self-Review (4b-2)

**Spec coverage (§5):**
- §5.1 swap/recovery file: location/permissions/filename/header/content (Tasks 2, 3, 4); cadence (Task 5); version-aware lifecycle (Task 6); hash-first recovery predicate + prompt (Tasks 7, 8). ✅
- §5.2 timer mechanics (`recv_timeout` + `next_deadline_ms`, swap dispatched as a job): Task 5. ✅
- §5.3 modal infra (generic mechanism + routing) for quit/external-mod/recovery: Tasks 1, 8 (+9, +7). ✅
- §5.4 external-mod detection (`Option<FileFingerprint>` matrix) + Reload/Overwrite/Save-as semantics (Save-as disabled in 4b): Task 9. ✅
- §5.5 panic dump (global last-good snapshot, `try_lock`, `0600`, dump routine tested directly): Task 10. ✅

**Spec §7 testing strategy mapping:** substrate/staleness (4b-1); version-aware save status + fingerprint matrix (Task 9, 4b-1 Task 9); save&quit waits/timeout (Task 8); swap header round-trip + cadence under fake clock + recovery matrix + delete-on-clean + `0600`/`0700` (Tasks 2–7); panic dump routine + try_lock-held skip (Task 10 — add a `dump_on_panic` no-deadlock test if desired); external-mod injected fingerprint (Task 9); determinism — every test uses `InlineExecutor`/injected `Clock`, no sleeps except the one `fingerprint`-mtime test (which uses a real 10 ms write gap; if that proves flaky on a coarse-mtime FS, swap it to set `stored_fp` directly).

**Placeholder scan:** mid-plan inert states are explicitly called out and finalized in a later task (Task 7's recovery keys → resolved in Task 8; Task 9's `Reload`/`Overwrite` referenced in Task 8's resolver → implemented in Task 9). No "TBD"/"add error handling" placeholders; all code blocks are concrete.

**Type consistency:** `Prompt`/`Choice`/`PromptAction`, `SwapHeader` fields, `swap::{fnv1a64, state_dir, swap_path, write_atomic, build_header, dispatch_swap_write, due, next_deadline_ms, delete, assess, RecoveryDecision}`, `recovery::{LAST_GOOD, record_snapshot, write_dump, dump_on_panic}`, `save::{overwrite_save, reload_from_disk, load_recovered, do_save}`, `Editor` new fields (`prompt`, `last_edit_at`, `last_swap_at`, `quit_after_save`, `pending_swap_body`) — used identically across tasks. The `pending_quit` field is removed in Task 8 (its test migrated to the modal).

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-wordcartel-04b2-crash-safety.md`. Execute **after** 4b-1 is merged.
