# H32+H33 Test-Environment Seam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give "get a scratch path" exactly one answer (a `test_support` seam replacing 15 duplicated helpers) and remove the workspace's only env mutation plus every test that reads the real process environment as an assertion oracle.

**Architecture:** Two seam functions (`scratch_path`/`scratch_dir`) over ONE crate-global `AtomicU64` counter in the existing `#[cfg(test)] test_support` module; legacy helpers become one-line delegations (call sites untouched). A new `pathx` module holds the pure `expand_tilde` and the `PlatformDirs { home, config_dir }` carrier; `load_with_fs` gains the carrier, `resolve_field` gains a `home` param, and the `set_var("HOME")` test block is deleted.

**Tech Stack:** Rust 2021 (3-crate workspace), `dirs` crate at production boundaries only, std-only test machinery. Spec: `docs/superpowers/specs/2026-07-25-h32h33-test-env-seam-design.md`.

## Global Constraints

- **Command-surface contract: N/A — does not touch the command surface.** No command, option, palette entry, menu item, or keybinding is added, removed, or altered.
- **Intermediate green:** EVERY commit leaves `cargo test --workspace` green and `cargo build`/`cargo test --no-run` warning-free for `wordcartel`. Never commit red.
- **Dependency edges:** T2/T3 need T1; T5, T6, T7 need T4; T6 needs T5. T2 ⊥ T4 (independent).
- **Seam contract values (reviewer checklist):** path format `wc-scratch-{pid}-{seq}-{label}` with label ALWAYS last; ONE shared `static SCRATCH_SEQ: AtomicU64`; the uniqueness-invariant test runs **≥32 threads**; after T7, `grep -rn "set_var\|remove_var" wordcartel/src wordcartel-core/src wordcartel-nlp/src wordcartel/tests wordcartel-core/tests --include="*.rs"` returns **nothing**.
- **TDD honesty:** genuine RED only where marked (T1 invariant test; T4 `expand_tilde` suite incl. `home == None`; T6 `default_dictionary_path`). Every step marked PIN is green-on-arrival — do NOT manufacture a red for a behavior-preserving migration; the named existing suite is the pin.
- **Gates (from spec §5):** workspace clippy clean (`cargo clippy --workspace --all-targets`); **no `cargo fmt` ever** — match neighbors by hand (4-space indent, ~100-char hand-wrapped lines, em-dash in prose comments, imports grouped by hand); no dead code (delete orphaned statics AND their now-unused `use std::sync::atomic::…` imports); doc-comment every public item, runnable `# Examples` on non-obvious public fns.
- For compile/usage questions on code you are editing, trust `cargo` + `grep`, never an editor "unused"/"undefined" hint. Anchor by symbol name; line numbers below are a courtesy and may have drifted.
- Every commit ends with the project trailers per `CLAUDE.md` (the `Co-Authored-By` line and the `Claude-Session:` URL from YOUR harness instructions — never invent one).
- Out of scope (do NOT touch): the ~105 loose inline `temp_dir()` sites (filed as H36); `wordcartel/tests/harper_ls_*.rs` (separate crates — cannot reach a `#[cfg(test)]` seam); env READS (`clipboard.rs`, `theme_resolve.rs`, `WCARTEL_SMOKE_PANIC`, `current_dir`); `fsx.rs::TEMP_SEQ` and `editor.rs::DocumentId::mint`'s `SEQ` (production statics, unrelated).

---

### Task 1: The scratch seam + uniqueness invariant (+ `nix_privileged` fold)

**Files:**
- Modify: `wordcartel/src/test_support.rs` (append seam + tests; edit `nix_privileged`)

**Interfaces:**
- Produces: `pub(crate) fn scratch_path(label: &str) -> PathBuf` (unique path, NOT created; label carries any extension) and `pub(crate) fn scratch_dir(label: &str) -> PathBuf` (unique dir, created, empty by construction). Both share `static SCRATCH_SEQ: AtomicU64`; format `wc-scratch-{pid}-{seq}-{label}`. T2/T3/T6/T7 call these as `crate::test_support::scratch_path` / `scratch_dir`.

- [ ] **Step 1: Write the failing invariant test** — append to the END of `wordcartel/src/test_support.rs` (the module is already `#[cfg(test)]`-only; a nested `mod tests` groups its own tests):

```rust
// ---------------------------------------------------------------------------
// H32: the scratch-path seam — the ONE answer to "get a scratch path" for every
// test module in this crate. Uniqueness = process id + a single crate-global
// counter; the invariant test below is the durable guardrail (H31's class).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam's uniqueness contract under the concurrency that surfaced H31:
    /// ≥32 threads interleaving both fns yield pairwise-distinct paths; every
    /// `scratch_dir` result exists and is empty at return; every `scratch_path`
    /// result does not exist at return.
    #[test]
    fn scratch_seam_is_collision_free_under_contention() {
        const THREADS: usize = 32;
        const PER: usize = 8;
        let mut handles = Vec::with_capacity(THREADS);
        for t in 0..THREADS {
            handles.push(std::thread::spawn(move || {
                let mut got = Vec::with_capacity(PER);
                for i in 0..PER {
                    if (t + i) % 2 == 0 {
                        let p = scratch_path("inv.md");
                        assert!(!p.exists(),
                            "scratch_path must not create: {}", p.display());
                        got.push(p);
                    } else {
                        let d = scratch_dir("inv");
                        assert!(std::fs::read_dir(&d).expect("scratch_dir exists")
                            .next().is_none(),
                            "scratch_dir must be empty at birth: {}", d.display());
                        got.push(d);
                    }
                }
                got
            }));
        }
        let mut all = std::collections::HashSet::new();
        let mut created = Vec::new();
        for h in handles {
            for p in h.join().expect("seam thread must not panic") {
                if p.is_dir() { created.push(p.clone()); }
                assert!(all.insert(p), "seam returned a duplicate path");
            }
        }
        assert_eq!(all.len(), THREADS * PER, "one distinct path per call");
        for d in created { let _ = std::fs::remove_dir_all(&d); }
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p wordcartel --lib test_support::tests::scratch_seam -- --nocapture`
Expected: COMPILE FAIL — `error[E0425]: cannot find function 'scratch_path' in this scope` (and `scratch_dir`). This is the genuine RED (new API).

- [ ] **Step 3: Implement the seam** — insert directly ABOVE the `mod tests` block from Step 1:

```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Crate-global scratch counter — the ONE source of path uniqueness for both seam fns.
/// One static, shared, deliberately: split or per-module counters are the aliasing
/// fragility the H32 census flagged (swap.rs / fsx.rs history).
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_name(label: &str) -> PathBuf {
    let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("wc-scratch-{}-{seq}-{label}", std::process::id()))
}

/// A unique scratch PATH under the system temp dir — NOT created. The label is a short
/// test tag and carries any extension: `scratch_path("bgsave.md")`.
pub(crate) fn scratch_path(label: &str) -> PathBuf { scratch_name(label) }

/// A unique scratch DIRECTORY under the system temp dir — created, and empty by
/// construction: the pid+seq pair has never been issued before, so no prior content
/// can exist (this subsumes every legacy remove-then-create dance).
pub(crate) fn scratch_dir(label: &str) -> PathBuf {
    let d = scratch_name(label);
    std::fs::create_dir_all(&d).expect("scratch_dir: create");
    d
}
```

Note: `test_support.rs` already has `use crate::fsx::{Fs, RealFs, WriteSync};` and other mid-file imports — place these two `use` lines with the seam block (mid-file `use` before the fns is the file's established pattern, see its FaultFs section). If `PathBuf` conflicts with an existing import, qualify as `std::path::PathBuf` instead.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p wordcartel --lib test_support::tests::scratch_seam`
Expected: PASS (1 test).

- [ ] **Step 5: Fold `nix_privileged`** — in the same file, its body currently begins:

```rust
    let d = std::env::temp_dir().join(format!("wc-priv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    if std::fs::create_dir_all(&d).is_err() { return false; }
```

Replace those three lines with (the FIXED pid-only name is the exact H31 shape if a second caller ever appears; `scratch_dir` panics on create failure where the old code returned `false` — acceptable: a test box that cannot create a temp dir cannot run this suite at all):

```rust
    let d = scratch_dir("priv");
```

The rest of the fn (chmod dance, cleanup `remove_dir_all`s, `#[allow(unreachable_code)]` tail) is unchanged. PIN — green on arrival, do not manufacture a red; the pin is `file_browser.rs`'s unreadable-dir test (its only caller):
`cargo test -p wordcartel --lib file_browser` → PASS.

- [ ] **Step 6: Full green + gates**

Run: `cargo test --workspace` → all green. `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/test_support.rs
git commit -m "h32: scratch_path/scratch_dir seam + 32-thread uniqueness invariant"
```

---

### Task 2: Migrate the 10 self-contained helpers (delegation bodies)

**Files:**
- Modify: `wordcartel/src/settings.rs`, `wordcartel/src/config.rs`, `wordcartel/src/state.rs`, `wordcartel/src/file_browser_commit.rs`, `wordcartel/src/fsx.rs` (only `unique_dir`), `wordcartel/src/plugin/load.rs`, `wordcartel/src/save.rs`, `wordcartel/src/session_restore.rs`, `wordcartel/src/jobs_apply.rs`

**Interfaces:**
- Consumes: `crate::test_support::{scratch_path, scratch_dir}` from Task 1.
- Produces: nothing new — every legacy helper keeps its NAME and signature; ~120 call sites across these modules are untouched.

PIN — every step here is behavior-preserving; green on arrival; do not manufacture a red. The pin for each module is its own existing suite (run per-module below, whole workspace at the end).

Each edit: replace the helper's BODY with the one-liner shown, delete its now-orphaned counter `static` (and fn-local `use std::sync::atomic::…` lines inside deleted bodies), then delete any module-level `use std::sync::atomic::{…};` import that has become unused — `cargo build -p wordcartel` (warning-free is the gate) is the arbiter of "unused", not an editor hint.

- [ ] **Step 1: `settings.rs::tempdir`** — current body (fn-local `static N: AtomicU64` + create). New:

```rust
    fn tempdir() -> PathBuf {
        crate::test_support::scratch_dir("settings")
    }
```

- [ ] **Step 2: `config.rs::tempdir`** — same shape as settings. New:

```rust
    fn tempdir() -> PathBuf {
        crate::test_support::scratch_dir("cfg")
    }
```

- [ ] **Step 3: `config.rs::scratch_cfg_path`** — keep its H31 explanatory comment ("Unique per call. Two call sites pass the same `name` … A shared path was H31: one test's remove_file deleted another test's file between its write and its read."), replace the body:

```rust
    fn scratch_cfg_path(name: &str) -> PathBuf {
        crate::test_support::scratch_path(&format!("cfg-{name}.toml"))
    }
```

- [ ] **Step 4: `state.rs::tmp`** — new body:

```rust
    fn tmp() -> PathBuf {
        crate::test_support::scratch_dir("state")
    }
```

- [ ] **Step 5: `file_browser_commit.rs::tmp`** — the legacy `let _ = std::fs::remove_dir_all(&d);` cleanup-first is dropped, subsumed by fresh-unique (spec §2.1). New body:

```rust
    fn tmp(label: &str) -> std::path::PathBuf {
        crate::test_support::scratch_dir(&format!("commit-{label}"))
    }
```

- [ ] **Step 6: `fsx.rs::unique_dir`** — (NOT `private_dir`: that uses the module-level `SEQ` and migrates in Task 3; this task deliberately touches only the fn-local-counter helper in this file). New body:

```rust
    fn unique_dir(label: &str) -> PathBuf {
        crate::test_support::scratch_dir(&format!("fsx-{label}"))
    }
```

- [ ] **Step 7: `plugin/load.rs::unique_plugin_dir`** — new body:

```rust
    fn unique_plugin_dir(label: &str) -> std::path::PathBuf {
        crate::test_support::scratch_dir(&format!("plug-{label}"))
    }
```

- [ ] **Step 8: `save.rs::scratch`** — its module `static SEQ: AtomicU32` has no other user; delete the static and the `use std::sync::atomic::{AtomicU32, Ordering};` line if now unused. New body:

```rust
    fn scratch() -> std::path::PathBuf {
        crate::test_support::scratch_path("bgsave.md")
    }
```

- [ ] **Step 9: `session_restore.rs::scratch`** — sole-user module static; same deletion rule. New body:

```rust
    fn scratch() -> std::path::PathBuf {
        crate::test_support::scratch_path("sessmig.md")
    }
```

- [ ] **Step 10: `jobs_apply.rs::quit_tmp`** — sole-user module static; same deletion rule. The legacy tag-first name ordering is dropped (cosmetic — nothing reads the path shape back). New body:

```rust
    fn quit_tmp(tag: &str) -> std::path::PathBuf {
        crate::test_support::scratch_path(&format!("c4-{tag}.md"))
    }
```

- [ ] **Step 11: Verify pins per module, then whole workspace**

Run each touched module's suite (cargo test takes ONE filter per invocation):
`for m in settings config state file_browser_commit fsx plugin save session_restore jobs_apply; do cargo test -p wordcartel --lib "$m" || break; done` → every run PASS.
Run: `cargo test --workspace` → green; `cargo build -p wordcartel` → zero warnings (this is what proves the import pruning was exactly right); `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 12: Commit**

```bash
git add -u wordcartel/src
git commit -m "h32: delegate the 10 self-contained scratch helpers to the seam"
```

---

### Task 3: Migrate the shared-static modules (helpers + 11 inline SEQ co-users)

**Files:**
- Modify: `wordcartel/src/file.rs`, `wordcartel/src/app.rs`, `wordcartel/src/swap.rs`, `wordcartel/src/fsx.rs` (`private_dir` + module `SEQ`)

**Interfaces:**
- Consumes: `crate::test_support::{scratch_path, scratch_dir}` from Task 1.
- Produces: nothing new — helper names/signatures unchanged; after this task ZERO legacy scratch statics remain (verification step 5).

PIN — behavior-preserving throughout; green on arrival; do not manufacture a red. Each module's own suite is the pin. Every inline edit below is located by its ENCLOSING TEST NAME, not line number. Where an inline site's next line is `std::fs::create_dir_all(&…)` / `.expect(…)` for the same path, DELETE that line too — `scratch_dir` already created it.

Label convention (deliberate — do not "fix"): inline co-user labels DROP any legacy `wc-`
prefix because the seam itself supplies `wc-scratch-{pid}-{seq}-`; keeping it would double
the marker (`wc-scratch-…-wc-export-confirmed`). Labels are the bare test tag; no test
reads the path shape back, and per-call uniqueness comes from the SEQ, not the label —
labels are readability only (they also happen to be unique within each module below).

- [ ] **Step 1: `file.rs`** — three users of the module `static SEQ: AtomicU32` (in `mod tests`, comment "Unique scratch path: pid + monotonic counter + a label."):
  1. Helper `scratch_path(label)` (module-local name; call sites keep it) — new body:

```rust
    fn scratch_path(label: &str) -> PathBuf {
        crate::test_support::scratch_path(&format!("file-{label}.txt"))
    }
```

  2. In test `save_atomic_bytes_roundtrip_no_litter`, replace

```rust
        let private_dir = std::env::temp_dir().join(format!(
            "wcartel-bytes-litter-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&private_dir).expect("create private temp subdir");
```

  with:

```rust
        let private_dir = crate::test_support::scratch_dir("bytes-litter");
```

  3. In test `no_temp_litter_after_save`, same shape (keep its explanatory comment about a private subdir): replace the `wcartel-littertest-…` block + its `create_dir_all` line with:

```rust
        let private_dir = crate::test_support::scratch_dir("littertest");
```

  Then delete `static SEQ: AtomicU32 = AtomicU32::new(0);`, its comment line, and prune `use std::sync::atomic::{AtomicU32, Ordering};` if now unused.
  Pin: `cargo test -p wordcartel --lib file::` → PASS.

- [ ] **Step 2: `app.rs`** — six users of the `mod tests` `static SEQ: AtomicU32`:
  1. Helper `quit_tmp(tag)` (tag-first ordering dropped, cosmetic) — new body:

```rust
    fn quit_tmp(tag: &str) -> std::path::PathBuf {
        crate::test_support::scratch_path(&format!("quit-{tag}.md"))
    }
```

  2. `tick_writes_swap_when_idle_elapsed_and_dirty`: replace the `wc-tick-swap-…` `doc_path` block with:

```rust
        let doc_path = crate::test_support::scratch_path("tick-swap.md");
```

  3. `exportdone_bytes_writes_file_beside_source`: replace the `wc-exportdone-…` `tmp_dir` block AND its `std::fs::create_dir_all(&tmp_dir).expect("create temp dir");` line with:

```rust
        let tmp_dir = crate::test_support::scratch_dir("exportdone");
```

  4. `exportdone_unconfirmed_refuses_when_target_appeared`: same shape, label `"export-toctou"`:

```rust
        let tmp_dir = crate::test_support::scratch_dir("export-toctou");
```

  5. `exportdone_confirmed_overwrites_existing_target`: same shape, label `"export-confirmed"`:

```rust
        let tmp_dir = crate::test_support::scratch_dir("export-confirmed");
```

  6. `exportdone_under_prompt_still_applies`: same shape, label `"exportdone-prompt"`:

```rust
        let tmp_dir = crate::test_support::scratch_dir("exportdone-prompt");
```

  Then delete `static SEQ: AtomicU32 = AtomicU32::new(0);` and prune `use std::sync::atomic::{AtomicU32, Ordering};` if now unused. (`app.rs` is a budgeted hub — these are all test-side lines, which the `module_budgets` counter strips; production size is untouched.)
  Pin: `cargo test -p wordcartel --lib app::` → PASS.

- [ ] **Step 3: `swap.rs`** — six users of the `mod tests` `static SEQ: AtomicU32`:
  1. Helper `scratch()` — new body:

```rust
    fn scratch() -> std::path::PathBuf {
        crate::test_support::scratch_path("swap.md")
    }
```

  2. Helper `unique_dir(label)` — new body:

```rust
    fn unique_dir(label: &str) -> std::path::PathBuf {
        crate::test_support::scratch_dir(&format!("h5-{label}"))
    }
```

  3. `find_orphan_scratch_swap_finds_dead_pid_and_skips_self` (keep its comment about needing a UNIQUE dir, not the shared state dir): replace the `wc-orphan-test-…` block + its `std::fs::create_dir_all(&dir).unwrap();` line with:

```rust
        let dir = crate::test_support::scratch_dir("orphan");
```

  4. `dispatch_swap_write_writes_a_recoverable_swap`: replace the `wc-dispatch-swap-…` `doc_path` block with:

```rust
        let doc_path = crate::test_support::scratch_path("dispatch-swap.md");
```

  5. `dispatch_swap_write_uses_the_injected_fs_and_a_failed_write_does_not_latch`: replace the `wc-dispatch-swap-fault-…` block with:

```rust
        let doc_path = crate::test_support::scratch_path("dispatch-swap-fault.md");
```

  6. `stale_path_swap_does_not_relatch_after_rekey`: the shared-seq old/new pair — the correlation is cosmetic (the test needs only two distinct unique paths). Replace

```rust
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let old_path = std::env::temp_dir().join(format!("wc-rekey-old-{}-{}.md", std::process::id(), seq));
        let new_path = std::env::temp_dir().join(format!("wc-rekey-new-{}-{}.md", std::process::id(), seq));
```

  with:

```rust
        let old_path = crate::test_support::scratch_path("rekey-old.md");
        let new_path = crate::test_support::scratch_path("rekey-new.md");
```

  Then delete `static SEQ: AtomicU32 = AtomicU32::new(0);` and prune `use std::sync::atomic::{AtomicU32, Ordering};` if now unused.
  Pin: `cargo test -p wordcartel --lib swap::` → PASS.

- [ ] **Step 4: `fsx.rs::private_dir`** — the module-level `static SEQ: AtomicU32` sole user (its doc comment "A private per-test dir under the system temp dir, so a glob for '.tmp' is not polluted…" stays — uniqueness preserves the purpose). New body:

```rust
    fn private_dir(label: &str) -> PathBuf {
        crate::test_support::scratch_dir(&format!("fsx-{label}"))
    }
```

  Delete `static SEQ: AtomicU32 = AtomicU32::new(0);` and prune the atomic import if now unused. (Optional, implementer's call per spec §2.4: `unique_dir` — already delegated in Task 2, same `mod tests`, now an identical body — may be collapsed into `private_dir` with call sites re-pointed; either state is conformant.)
  Pin: `cargo test -p wordcartel --lib fsx::` → PASS.

- [ ] **Step 5: Verify the end state**

Run: `grep -rnE "static [A-Z_]*SEQ" wordcartel/src`
Expected: EXACTLY three survivors, all production and out of scope — `editor.rs`
(`DocumentId::mint`'s fn-local `SEQ`), `fsx.rs` (`TEMP_SEQ`, atomic-write temp naming),
and `clipboard.rs` (`PASTE_SEQ`, paste-id sequence). Any OTHER hit is an unmigrated
legacy scratch static — the task is not done.
Run: `cargo test --workspace` → green; `cargo build -p wordcartel` → zero warnings; `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 6: Commit**

```bash
git add -u wordcartel/src
git commit -m "h32: fold the shared-static modules (file/app/swap/fsx) into the seam"
```

---

### Task 4: The `pathx` module — `expand_tilde` + `PlatformDirs`

**Files:**
- Create: `wordcartel/src/pathx.rs`
- Modify: `wordcartel/src/lib.rs` (one line: `pub mod pathx;` — insert after `pub mod panicx;` at the top group, matching the `-x` convention neighbors)

**Interfaces:**
- Produces: `pub fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf` and `pub struct PlatformDirs { pub home: Option<PathBuf>, pub config_dir: Option<PathBuf> }` with `pub fn from_env() -> Self`. T5 consumes both; T7 consumes `expand_tilde`. `pub` visibility is deliberate: it keeps this additive commit warning-free (a `pub` lib item with no callers yet is not dead code) — do NOT downgrade to `pub(crate)`.

- [ ] **Step 1: Create the module with tests and a deliberately naive stub** — write `wordcartel/src/pathx.rs` in full:

```rust
//! pathx — pure tilde expansion and the platform-dirs carrier.
//!
//! The H33 seam: `dirs::*` is read ONCE at a production boundary (`PlatformDirs::from_env`,
//! or an inline `dirs::home_dir()` at a caller that IS the boundary) and passed down as
//! explicit data. Pure code and tests below the boundary never read the process
//! environment.

use std::path::{Path, PathBuf};

/// Expand a tilde against an EXPLICIT home directory — the pure core of every `~` site.
///
/// - `"~"` → `home`, or the literal `"~"` when `home` is `None`.
/// - `"~/rest"` → `home/rest`, or the literal input when `home` is `None`.
/// - anything else → `PathBuf::from(text)`, verbatim (no mid-string expansion).
///
/// `home` is `dirs::home_dir()` at every production boundary and an injected temp dir in
/// tests — no caller of this function reads the process environment.
///
/// # Examples
///
/// ```
/// use std::path::{Path, PathBuf};
/// use wordcartel::pathx::expand_tilde;
///
/// let home = Path::new("/home/w");
/// assert_eq!(expand_tilde("~/notes.md", Some(home)), PathBuf::from("/home/w/notes.md"));
/// assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/home/w"));
/// assert_eq!(expand_tilde("~/notes.md", None), PathBuf::from("~/notes.md"));
/// assert_eq!(expand_tilde("plain.md", Some(home)), PathBuf::from("plain.md"));
/// ```
pub fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf {
    // Step-1 stub — replaced in Step 3.
    PathBuf::from(text)
}

/// Platform directories resolved ONCE at a production boundary and passed down — the
/// injection carrier that keeps `dirs::*` reads out of pure code and out of tests. A dumb
/// carrier on purpose: tests construct it literally with explicit paths; accessor ceremony
/// would fight the point.
///
/// # Examples
///
/// ```
/// use wordcartel::pathx::PlatformDirs;
///
/// // A test injects explicit dirs; production calls `PlatformDirs::from_env()`.
/// let dirs = PlatformDirs { home: Some("/home/w".into()), config_dir: None };
/// assert_eq!(dirs.home.as_deref(), Some(std::path::Path::new("/home/w")));
/// ```
pub struct PlatformDirs {
    pub home: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

impl PlatformDirs {
    /// Resolve from the real environment — production boundaries only; tests construct
    /// the struct literally with explicit paths. (Deliberately no unit test: this fn IS
    /// the env read, and asserting on it would recreate the oracle coupling H33 removes.)
    pub fn from_env() -> Self {
        PlatformDirs { home: dirs::home_dir(), config_dir: dirs::config_dir() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_slash_joins_the_injected_home() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("~/a/b.md", Some(home)), PathBuf::from("/inj/home/a/b.md"));
    }

    #[test]
    fn bare_tilde_is_the_injected_home_itself() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/inj/home"));
    }

    #[test]
    fn no_home_falls_back_to_the_literal_input() {
        // The never-before-tested branches: every legacy site fell back to the literal
        // text when the platform had no resolvable home. Asserted here, unconditionally.
        assert_eq!(expand_tilde("~", None), PathBuf::from("~"));
        assert_eq!(expand_tilde("~/a.md", None), PathBuf::from("~/a.md"));
    }

    #[test]
    fn non_tilde_text_passes_through_verbatim() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("plain/rel.md", Some(home)), PathBuf::from("plain/rel.md"));
        assert_eq!(expand_tilde("/abs/p.md", Some(home)), PathBuf::from("/abs/p.md"));
        assert_eq!(expand_tilde("", Some(home)), PathBuf::from(""));
        // No mid-string expansion — only a LEADING tilde means home.
        assert_eq!(expand_tilde("a/~/b", Some(home)), PathBuf::from("a/~/b"));
    }
}
```

Also add to `wordcartel/src/lib.rs`, directly after the `pub mod panicx;` line:

```rust
pub mod pathx;
```

- [ ] **Step 2: Run tests to verify the genuine RED**

Run: `cargo test -p wordcartel --lib pathx`
Expected: FAIL — `tilde_slash_joins_the_injected_home` and `bare_tilde_is_the_injected_home_itself` fail (the stub passes text through); `no_home_falls_back_to_the_literal_input` and `non_tilde_text_passes_through_verbatim` pass (passthrough IS their contract). 2 failed, 2 passed.

- [ ] **Step 3: Implement `expand_tilde` for real** — replace the stub body:

```rust
pub fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf {
    if text == "~" {
        return home.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(text));
    }
    PathBuf::from(text)
}
```

(Delete the "Step-1 stub" comment line.)

- [ ] **Step 4: Run tests + doc-tests to verify they pass**

Run: `cargo test -p wordcartel --lib pathx` → 4 passed.
Run: `cargo test -p wordcartel --doc pathx` → doc-examples pass.

- [ ] **Step 5: Full green + commit**

Run: `cargo test --workspace` → green; `cargo clippy --workspace --all-targets` → clean.

```bash
git add wordcartel/src/pathx.rs wordcartel/src/lib.rs
git commit -m "h33: pathx — pure expand_tilde + the PlatformDirs carrier"
```

---

### Task 5: Thread the carrier through `load_with_fs`

**Files:**
- Modify: `wordcartel/src/config.rs` (`load`, `load_with_fs` signature + both tilde sites, the `config_over_cap_degrades_like_an_unreadable_file` test)
- Modify: `wordcartel/src/app.rs` (one production line in `run`: the `xdg` resolution)

**Interfaces:**
- Consumes: `crate::pathx::{expand_tilde, PlatformDirs}` from Task 4.
- Produces: `pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf], pdirs: &crate::pathx::PlatformDirs) -> (Config, Vec<String>)`. T6's oracle tests call it with an injected carrier. `pub fn load(paths: &[PathBuf])` keeps its signature (so its ~30 test callers and 5 production call sites are untouched).

PIN — green on arrival (the behavior is unchanged; only the home SOURCE moves); the whole config suite is the pin. The parameter is named `pdirs` because the `dirs` CRATE is in scope in `config.rs` — do not shadow it.

- [ ] **Step 1: Change the signatures** — in `config.rs`:

`load` currently:

```rust
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    // fs-chokepoint-allow: (w) the `RealFs` wrapper itself — its `*_with_fs` seam is what injected callers use
    load_with_fs(&crate::fsx::RealFs, paths)
}
```

becomes (the comment stays — the carrier extends the wrapper's exact documented role):

```rust
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    // fs-chokepoint-allow: (w) the `RealFs` wrapper itself — its `*_with_fs` seam is what injected callers use
    load_with_fs(&crate::fsx::RealFs, paths, &crate::pathx::PlatformDirs::from_env())
}
```

`load_with_fs`'s signature line becomes:

```rust
pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf],
    pdirs: &crate::pathx::PlatformDirs) -> (Config, Vec<String>) {
```

- [ ] **Step 2: Re-point the diagnostics-dictionary tilde site** — inside `load_with_fs`, replace

```rust
        if let Some(s) = raw.diagnostics.dictionary {
            // Fix A7: expand a leading `~/` (or bare `~`) to the home directory so
            // paths like `~/foo/dict.txt` work correctly.  Without expansion a raw
            // PathBuf would write to a literal `~` directory.
            let expanded = if s == "~" {
                dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"))
            } else if let Some(rest) = s.strip_prefix("~/") {
                dirs::home_dir()
                    .map(|h| h.join(rest))
                    .unwrap_or_else(|| std::path::PathBuf::from(&s))
            } else {
                std::path::PathBuf::from(&s)
            };
            cfg.diagnostics.dictionary = Some(expanded);
        }
```

with (full delegation — `expand_tilde`'s else-branch IS `PathBuf::from`, an exact behavioral match):

```rust
        if let Some(s) = raw.diagnostics.dictionary {
            // Fix A7: expand a leading `~/` (or bare `~`) to the home directory so paths
            // like `~/foo/dict.txt` work correctly — via the pathx seam, against the home
            // the CALLER resolved (H33: no env read below the boundary).
            cfg.diagnostics.dictionary =
                Some(crate::pathx::expand_tilde(&s, pdirs.home.as_deref()));
        }
```

- [ ] **Step 3: Re-point the theme-file tilde site** — replace the `resolved_file` closure

```rust
        let resolved_file = rt.file.as_ref().map(|s| {
            if s == "~" {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
            } else if let Some(rest) = s.strip_prefix("~/") {
                dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(s))
            } else {
                let pb = PathBuf::from(s);
                if pb.is_absolute() { pb } else { layer_dir.join(pb) }
            }
        });
```

with (the non-tilde arm — absolute-or-layer-relative — is this site's OWN policy and stays):

```rust
        let resolved_file = rt.file.as_ref().map(|s| {
            if s == "~" || s.starts_with("~/") {
                crate::pathx::expand_tilde(s, pdirs.home.as_deref())
            } else {
                let pb = PathBuf::from(s);
                if pb.is_absolute() { pb } else { layer_dir.join(pb) }
            }
        });
```

- [ ] **Step 4: Fix the one direct `load_with_fs` test caller** — in `config_over_cap_degrades_like_an_unreadable_file`, replace

```rust
        let (cfg, warns) = load_with_fs(&crate::fsx::RealFs, std::slice::from_ref(&p));
```

with (env irrelevance made explicit — a size-cap test needs no platform dirs):

```rust
        let (cfg, warns) = load_with_fs(&crate::fsx::RealFs, std::slice::from_ref(&p),
            &crate::pathx::PlatformDirs { home: None, config_dir: None });
```

- [ ] **Step 5: The `app.rs` boundary line** — in `run`, replace

```rust
    let xdg = dirs::config_dir();
```

with (this is what gives `PlatformDirs::config_dir` its production consumer — the same app boundary that already passes `xdg` down into `config_layer_paths_with_fs`; the downstream `xdg.as_deref()` / `overrides_path` lines are untouched):

```rust
    let xdg = crate::pathx::PlatformDirs::from_env().config_dir;
```

- [ ] **Step 6: Verify the pin, full green**

Run: `cargo test -p wordcartel --lib config` → PASS (all existing config tests green — including the three oracle tests, still on their old oracles until Task 6).
Run: `cargo test --workspace` → green; `cargo build -p wordcartel` → zero warnings; `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 7: Commit**

```bash
git add -u wordcartel/src
git commit -m "h33: load_with_fs carries PlatformDirs; tilde sites go through pathx"
```

---

### Task 6: The oracle tests go unconditional (+ `default_dictionary_path`)

**Files:**
- Modify: `wordcartel/src/config.rs` (extract `default_dictionary_path`; rewrite the three oracle tests)

**Interfaces:**
- Consumes: `load_with_fs(fs, paths, pdirs)` from Task 5; `PlatformDirs` from Task 4; `crate::test_support::scratch_dir` from Task 1.
- Produces: `pub fn default_dictionary_path(config_dir: Option<&Path>) -> Option<PathBuf>` in `config.rs` (`use std::path::Path` may need adding to the import group — `cargo build` arbitrates).

- [ ] **Step 1: Write the failing `default_dictionary_path` test** — add to `config.rs`'s `mod tests`:

```rust
    /// The pure default-dictionary rule, asserted UNCONDITIONALLY against an injected
    /// config dir — no process-env oracle, no vacuous skip on exotic platforms.
    #[test]
    fn default_dictionary_path_joins_the_injected_config_dir() {
        let d = std::path::Path::new("/inj/cfg");
        assert_eq!(default_dictionary_path(Some(d)),
            Some(std::path::PathBuf::from("/inj/cfg/wordcartel/dictionary.txt")));
        assert_eq!(default_dictionary_path(None), None,
            "no config dir → no default dictionary");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p wordcartel --lib config::tests::default_dictionary_path_joins`
Expected: COMPILE FAIL — `error[E0425]: cannot find function 'default_dictionary_path'`. Genuine RED (new API).

- [ ] **Step 3: Extract the pure fn and delegate `Default`** — beside `impl Default for DiagnosticsConfig`, add:

```rust
/// `<config_dir>/wordcartel/dictionary.txt`, or `None` when the platform has no config
/// dir — the pure rule behind `DiagnosticsConfig::default()`'s dictionary field.
pub fn default_dictionary_path(config_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    config_dir.map(|d| d.join("wordcartel").join("dictionary.txt"))
}
```

and change the `Default` impl's dictionary line from

```rust
        let dictionary = dirs::config_dir().map(|d| d.join("wordcartel").join("dictionary.txt"));
```

to (the `Default` trait has no injection point by nature — this ONE production `dirs::config_dir()` read remains, per spec §3.5.3; keep the Fix-A7 comment above it):

```rust
        let dictionary = default_dictionary_path(dirs::config_dir().as_deref());
```

- [ ] **Step 4: Run the new test to verify it passes**

Run: `cargo test -p wordcartel --lib config::tests::default_dictionary_path_joins` → PASS.

- [ ] **Step 5: Rewrite `diagnostics_default_dictionary_is_not_none`** — PIN for the wiring (green on arrival); the `if let`/else-comment scaffolding dies. Full replacement (keep the existing `///` doc comment above it):

```rust
    #[test]
    fn diagnostics_default_dictionary_is_not_none() {
        // The RULE is pinned unconditionally by `default_dictionary_path_joins_the_
        // injected_config_dir`; this test pins only the Default WIRING — that the field
        // actually routes through that rule. Conditional by necessity: `Default` has no
        // injection point, so on a platform with no config dir there is nothing to wire.
        let cfg = DiagnosticsConfig::default();
        assert_eq!(cfg.dictionary,
            default_dictionary_path(dirs::config_dir().as_deref()),
            "Default::default must delegate its dictionary to default_dictionary_path");
    }
```

- [ ] **Step 6: Rewrite the two tilde oracle tests** — PINS, green on arrival, do not manufacture reds; the change is the ORACLE'S SOURCE (injected home, mandatory assertions — the `if let Some(home) = dirs::home_dir()` guards die). Full replacements (keep each test's existing `///` doc comment):

```rust
    #[test]
    fn dictionary_tilde_is_expanded() {
        let dir = tempdir();
        let home = crate::test_support::scratch_dir("cfg-home");
        let p = dir.join("c.toml");
        std::fs::write(&p, "[diagnostics]\ndictionary = \"~/foo/dict.txt\"\n").unwrap();
        let pdirs = crate::pathx::PlatformDirs { home: Some(home.clone()), config_dir: None };
        let (cfg, warns) = load_with_fs(&crate::fsx::RealFs, &[p], &pdirs);
        assert!(warns.is_empty(), "tilde path must not produce warnings");
        // Asserted UNCONDITIONALLY against the INJECTED home — the test owns the answer;
        // no process-env oracle, no vacuous skip on a homeless container.
        assert_eq!(cfg.diagnostics.dictionary, Some(home.join("foo").join("dict.txt")),
            "~/foo/dict.txt must expand to <home>/foo/dict.txt, not a literal ~");
        // Belt: the stored path never begins with a literal tilde byte.
        let first = cfg.diagnostics.dictionary.as_ref().expect("asserted Some above")
            .to_string_lossy();
        assert!(!first.starts_with('~'),
            "expanded dictionary path must not start with a literal tilde, got: {first}");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

```rust
    #[test]
    fn dictionary_bare_tilde_expands_to_home() {
        let dir = tempdir();
        let home = crate::test_support::scratch_dir("cfg-bare-home");
        let p = dir.join("c.toml");
        std::fs::write(&p, "[diagnostics]\ndictionary = \"~\"\n").unwrap();
        let pdirs = crate::pathx::PlatformDirs { home: Some(home.clone()), config_dir: None };
        let (cfg, _warns) = load_with_fs(&crate::fsx::RealFs, &[p], &pdirs);
        assert_eq!(cfg.diagnostics.dictionary, Some(home.clone()),
            "bare ~ must expand to the injected home, unconditionally asserted");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 7: Verify — no config test reads the process env as an oracle**

Run: `cargo test -p wordcartel --lib config` → PASS.
Run: `grep -n "dirs::home_dir\|dirs::config_dir" wordcartel/src/config.rs`
Expected: exactly TWO hits, both CODE, both `dirs::config_dir()`, both sanctioned — the
`Default` impl's delegation line (`default_dictionary_path(dirs::config_dir().as_deref())`)
and the same expression in `diagnostics_default_dictionary_is_not_none`'s wiring pin
(which compares two productions of the SAME source, not env-as-ground-truth). ZERO
`dirs::home_dir()` hits remain in this file — the T5 carrier replaced the tilde sites, and
the rewritten test comments deliberately avoid the literal token so this grep stays exact.
Run: `cargo test --workspace` → green; `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 8: Commit**

```bash
git add -u wordcartel/src
git commit -m "h33: oracle tests assert against injected dirs; default_dictionary_path extracted"
```

---

### Task 7: `resolve_field` home param, `expand_path` re-point, the `set_var` dies

**Files:**
- Modify: `wordcartel/src/file_browser_commit.rs` (`resolve_field` + its production call in `classify_destination_enter` + two tests)
- Modify: `wordcartel/src/prompts.rs` (`expand_path` body)

**Interfaces:**
- Consumes: `crate::pathx::expand_tilde` from Task 4; `tmp(label)` (the Task-2 delegation) in `file_browser_commit.rs` tests.
- Produces: `pub(crate) fn resolve_field(dir: &Path, field: &str, home: Option<&Path>) -> PathBuf`. `classify_destination_enter`'s own signature is UNCHANGED (so its internal caller and `file_browser::footer_target` are untouched).

- [ ] **Step 1: `resolve_field` gains the parameter** — full replacement (doc comment's rule list stays; bare `~` deliberately still falls through to `dir.join` — spec §3.1 preserves this; unifying it would be a user-visible change filed separately if ever wanted):

```rust
pub(crate) fn resolve_field(dir: &Path, field: &str, home: Option<&Path>) -> PathBuf {
    let t = field.trim();
    if t.starts_with("~/") {
        return crate::pathx::expand_tilde(t, home);
    }
    let p = PathBuf::from(t);
    if p.is_absolute() { p } else { dir.join(p) }
}
```

- [ ] **Step 2: The production boundary** — in `classify_destination_enter`, replace

```rust
    let resolved = resolve_field(dir, trimmed);
```

with:

```rust
    let home = dirs::home_dir();
    let resolved = resolve_field(dir, trimmed, home.as_deref());
```

- [ ] **Step 3: Update the non-tilde test** — in `a_bare_relative_field_resolves_against_fb_dir_not_the_process_cwd`, the two calls pass `None` (proving env independence):

```rust
        assert_eq!(resolve_field(&d, "chapter.md", None), d.join("chapter.md"));
        assert_eq!(resolve_field(&d, "drafts/ch1.md", None), d.join("drafts/ch1.md"),
            "a relative path WITH segments also resolves under fb.dir");
```

- [ ] **Step 4: Rewrite the `set_var` test** — full replacement of `absolute_and_home_relative_fields_are_honoured`. The save/set/restore block, `var_os` read, and BOTH "Edition 2021: `set_var` is safe here" caveat comments are deleted; the mandatory assertion — the entire point of the original mutation — survives on injection. PIN — green on arrival; the assertion is the pin:

```rust
    #[test]
    fn absolute_and_home_relative_fields_are_honoured() {
        // The `~/` assertion is MANDATORY, not conditional. It was once guarded by
        // `if let Some(home) = dirs::home_dir()` (a vacuous pass on any container without
        // a resolvable home), then made mandatory by mutating $HOME — the workspace's only
        // env mutation, and `unsafe` from edition 2024. H33: the test OWNS the home answer
        // by passing it, so nothing is mutated and the assertion still cannot vanish.
        //
        // FAIL-VERIFY: delete the `~/` arm from `resolve_field`, watch this fail.
        let d = tmp("resolve-abs");
        assert_eq!(resolve_field(&d, "/etc/hosts", None),
            std::path::PathBuf::from("/etc/hosts"));

        let home = tmp("resolve-home");
        let got = resolve_field(&d, "~/notes.md", Some(&home));
        assert_eq!(got, home.join("notes.md"),
            "`~/` expands against the injected home dir, unconditionally asserted");

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&d);
    }
```

- [ ] **Step 5: Re-point `expand_path`** — in `prompts.rs`, full replacement (its OWN boundary: it resolves `dirs::home_dir()` inline; note honestly it has zero callers and zero tests today — this edit is pure dedup of the fourth duplicated site):

```rust
/// Expand a user-typed path: `~/` prefix → home dir; relative → joined onto cwd.
/// The `~` arm routes through `pathx::expand_tilde` (H33) — home is resolved HERE, at
/// the production boundary; a bare `~` deliberately keeps its legacy fall-through.
pub fn expand_path(text: &str) -> std::path::PathBuf {
    let expanded = if text.starts_with("~/") {
        crate::pathx::expand_tilde(text, dirs::home_dir().as_deref())
    } else { std::path::PathBuf::from(text) };
    if expanded.is_absolute() { expanded }
    else { std::env::current_dir().map(|d| d.join(&expanded)).unwrap_or(expanded) }
}
```

- [ ] **Step 6: Verify the H33 end state**

Run: `cargo test -p wordcartel --lib file_browser_commit prompts` → PASS.
Run: `grep -rn "set_var\|remove_var" wordcartel/src wordcartel-core/src wordcartel-nlp/src wordcartel/tests wordcartel-core/tests --include="*.rs"`
Expected: NO OUTPUT — the workspace's only env mutation is gone; the next edition-2024 bump has no `unsafe`-env blocker.
Run: `cargo test --workspace` → green; `cargo build -p wordcartel` → zero warnings; `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 7: Commit**

```bash
git add -u wordcartel/src
git commit -m "h33: resolve_field takes an injected home; the last set_var dies"
```

---

## Pre-merge (for the finishing pass, not a task)

Both final gates per the project pipeline (Fable whole-branch probe + Codex pre-merge GO/NO-GO), then: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `scripts/smoke/run.sh` (quote the one-line summary verbatim — advisory), backlog: H32 + H33 → `shipped` in `backlog.toml`, prose sections moved to `docs/backlog-archive.md` with `doc =` repointed, `scripts/backlog bless` (H36 stays open; its `depends_on = ["H32"]` is satisfied).
