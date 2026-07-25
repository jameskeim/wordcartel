# H32+H33 — tests get their environment from a seam, not the process

**Effort:** combined H32 (scratch-path seam) + H33 (HOME/env seam), branch
`effort-h32h33-test-env-seam` off `main`. Both items spun out of H31 (the config
test-collision flake, shipped 2026-07-20). One sentence governs both halves: **tests should
get their environment from a seam, not from the process** — H32 gives "get a scratch path"
exactly one answer; H33 removes the workspace's only env mutation and every test that reads
the real process environment as an assertion oracle.

**Provenance:** grounded forks doc `scratchpad/h32h33/h32h33-forks.md`; all six forks ruled
by the human 2026-07-25. This spec implements the ruled decisions; every signature and call
site below was re-verified against the branch source at authoring time. Anchors are symbol
names; line numbers are a courtesy and drift.

**Command-surface contract: N/A — does not touch the command surface.** The effort touches
`test_support.rs`, test modules, one new small production module, and three internal
signatures (`load_with_fs`, `resolve_field`, plus the new `expand_tilde`). No command,
user-settable option, palette entry, menu item, or keybinding hint is added, removed, or
altered; the registry and keymap are untouched. The plan must restate this.

---

## 1. Grounded inventory (verified against source)

### 1.1 The 15 scratch-path helpers (H32)

All fifteen re-read on this branch. Every one is the same idiom — `std::env::temp_dir()`
joined with a `format!` of `process::id()` + a static atomic counter (+ optional label) —
varying only in create-vs-name-only, `AtomicU32` vs `AtomicU64`, label position, extension,
and one cleanup-first case (`file_browser_commit::tmp`'s `remove_dir_all` before create).
The idiom is CORRECT everywhere; this is duplication, not a bug (the filed reasoning).

| # | Module | Helper | Shape | Counter static |
|---|--------|--------|-------|----------------|
| 1 | `file.rs` | `scratch_path(label)` | path, `.txt` | module `SEQ: AtomicU32` (**shared** — see 1.2) |
| 2 | `settings.rs` | `tempdir()` | dir | fn-local `N: AtomicU64` |
| 3 | `save.rs` | `scratch()` | path, `.md` | module `SEQ: AtomicU32` (sole user) |
| 4 | `config.rs` | `tempdir()` | dir | fn-local `N: AtomicU64` |
| 5 | `config.rs` | `scratch_cfg_path(name)` | path, `.toml` | fn-local `N: AtomicU64` (H31's own fix; its comment: "Mirrors `tempdir()`'s idiom") |
| 6 | `file_browser_commit.rs` | `tmp(label)` | dir, cleanup-first | fn-local `N: AtomicU32` |
| 7 | `fsx.rs` | `private_dir(label)` | dir | module `SEQ: AtomicU32` (sole user; production `TEMP_SEQ` in `create_temp` is a SEPARATE static — untouched) |
| 8 | `fsx.rs` | `unique_dir(label)` | dir | fn-local `N: AtomicU32` (SAME `mod tests` as `private_dir`; keeps its own fn-local counter) |
| 9 | `state.rs` | `tmp()` | dir | fn-local `N: AtomicU64` |
| 10 | `swap.rs` | `scratch()` | path, `.md` | module `SEQ: AtomicU32` (**shared** — see 1.2) |
| 11 | `swap.rs` | `unique_dir(label)` | dir | same shared `SEQ` |
| 12 | `app.rs` | `quit_tmp(tag)` | path, `.md`, tag-first | module `SEQ: AtomicU32` in `mod tests` (**shared** — see 1.2) |
| 13 | `jobs_apply.rs` | `quit_tmp(tag)` | path, `.md`, tag-first | module `SEQ: AtomicU32` (sole user) |
| 14 | `session_restore.rs` | `scratch()` | path, `.md` | module `SEQ: AtomicU32` (sole user) |
| 15 | `plugin/load.rs` | `unique_plugin_dir(label)` | dir | fn-local `N: AtomicU32` |

Aggregate call-site volume through the helpers: roughly 150–160.

### 1.2 Inline SEQ co-users (must migrate with their static)

Three modules' helper statics also feed inline `temp_dir().join(format!(…SEQ.fetch_add…))`
constructions. Deleting the static without folding these would break the build; folding them
IS in scope (decision 2). By enclosing test name:

- **`file.rs`** (2): `save_atomic_bytes_roundtrip_no_litter`, `no_temp_litter_after_save` —
  each builds a private dir then `create_dir_all`s it.
- **`app.rs`** (5): `tick_writes_swap_when_idle_elapsed_and_dirty` (a `.md` doc path);
  `exportdone_bytes_writes_file_beside_source`,
  `exportdone_unconfirmed_refuses_when_target_appeared`,
  `exportdone_confirmed_overwrites_existing_target`,
  `exportdone_under_prompt_still_applies` (each a dir + `create_dir_all`).
- **`swap.rs`** (4): `find_orphan_scratch_swap_finds_dead_pid_and_skips_self` (dir);
  `dispatch_swap_write_writes_a_recoverable_swap`,
  `dispatch_swap_write_uses_the_injected_fs_and_a_failed_write_does_not_latch` (doc paths);
  `stale_path_swap_does_not_relatch_after_rekey` (draws ONE seq and names an old/new path
  PAIR with it — the correlation is cosmetic; two independent seam calls preserve the only
  property the test needs, two distinct unique paths).

All other inline `temp_dir()` uses in these modules are pid-only (no SEQ) and are OUT of
scope (§1.4).

### 1.3 Existing seam home

`wordcartel/src/lib.rs` declares `#[cfg(test)] pub(crate) mod test_support;` (line 82). The
module is the crate's sanctioned home for shared test helpers (its own C5-era comments say
so) and currently has NO scratch-path helper. Its `nix_privileged()` builds a FIXED
`wc-priv-{pid}` path with a remove/create/chmod dance — one caller today
(`file_browser.rs`, the unreadable-dir test), so no live collision, but the fixed name is
the exact H31 shape if a second caller ever appears; it folds into the seam (§2.3). The
`fs_chokepoint` scanner whole-file-exempts `test_support.rs` and `e2e.rs`, so the seam's
`create_dir_all` needs no exemption marker. `module_budgets` budgets only
`app.rs`/`render.rs`/`timers.rs`/`plugin/host.rs`/`plugin/pump.rs` — none gains production
lines here.

### 1.4 Out of scope (declared, tracked)

- **The ~105 loose inline `temp_dir()` constructions** across ~30 files of `wordcartel/src`
  test code (grounded count: ~141 `temp_dir()` occurrences minus the 15 helper bodies,
  `nix_privileged`, and a handful of non-path uses — browse-dir seeds, comments, one
  `starts_with` assertion). Individually correct (pid-unique, one label per test); no decay.
  **Filed as H36** (`depends_on = ["H32"]`) — that item, not this effort, is their tracked
  home. This spec deliberately does not touch them.
- **The 2 integration-test sites** (`wordcartel/tests/harper_ls_probe.rs`,
  `wordcartel/tests/harper_ls_integration.rs`) — one pid-suffixed dir each inside
  `#[ignore]`d tests. Integration tests compile as separate crates and **cannot reach a
  `#[cfg(test)] pub(crate)` seam**; they stay as-is. This crate boundary is accepted and
  recorded here.
- **Env READS** (`clipboard.rs` PATH/Wayland detection, `theme_resolve.rs`
  NO_COLOR/COLORTERM/TERM, `app.rs` WCARTEL_SMOKE_PANIC, `prompts.rs` `current_dir`) —
  reads remain safe under edition 2024; H33 removes env *mutation* and env-as-*oracle* only.
- **`wordcartel-core` / `wordcartel-nlp` / fuzz targets** — zero `temp_dir()` or env use;
  confirmed untouched.

### 1.5 H33 grounded state

- The `set_var("HOME")`/`remove_var("HOME")` block in
  `file_browser_commit.rs::absolute_and_home_relative_fields_are_honoured` is the **only env
  mutation in the workspace** (grep across all crates' `src/` + `tests/`). All three crates
  are `edition = "2021"`; `set_var` becomes `unsafe` at the 2024 bump — removing this block
  clears the workspace's only edition-2024 env blocker. The two "Edition 2021: `set_var` is
  safe here" caveat comments at the site are deleted with it.
- The tilde-expansion logic is duplicated at **four** production sites: `config.rs::
  load_with_fs` ×2 (diagnostics dictionary; theme `file`), `file_browser_commit.rs::
  resolve_field`, `prompts.rs::expand_path`. The `Fs` trait carries no home-dir method (it
  is file I/O only) and per decision 3 gains none.
- Sites that handle bare `~`: only the two `load_with_fs` sites. `resolve_field` and
  `expand_path` expand **only** the `~/` prefix — a bare `~` falls through to their
  relative-join behavior. This nonuniformity is **preserved** (§3.1); unifying it would be a
  user-visible behavior change outside this effort's charter.
- Prior art for the mechanism (the house pattern): `app.rs::run` already resolves
  `dirs::config_dir()` once and passes it down (`config_layer_paths_with_fs(…, xdg.as_deref(), …)`);
  `swap.rs::state_dir` is already `#[cfg(test)]`-diverted to a temp base.
- `prompts::expand_path` is `pub` with **zero callers and zero tests** today (two comments in
  `file_browser_commit.rs` reference it only as a deliberate contrast). Its re-point is pure
  dedup; stated honestly so the plan doesn't invent coverage claims around it.
- The three `config.rs` oracle tests: `diagnostics_default_dictionary_is_not_none` and
  `dictionary_bare_tilde_expands_to_home` **vacuously skip** when
  `dirs::config_dir()`/`home_dir()` is `None`; `dictionary_tilde_is_expanded` has one
  unconditional backstop (no literal `~` byte). All three read the real process env as
  oracle. End state: all assertions mandatory, zero env reads (§3.4, §3.5).

---

## 2. Design — H32: the scratch seam

### 2.1 The seam (decision 1, 6)

Two functions in `wordcartel/src/test_support.rs`, sharing ONE module-level counter:

```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Crate-global scratch counter — the ONE source of path uniqueness for both seam fns.
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_name(label: &str) -> PathBuf {
    let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("wc-scratch-{}-{seq}-{label}", std::process::id()))
}

/// A unique scratch PATH under the system temp dir — NOT created. The label is a short
/// test tag and carries any extension (`scratch_path("bgsave.md")`).
pub(crate) fn scratch_path(label: &str) -> PathBuf { scratch_name(label) }

/// A unique scratch DIRECTORY under the system temp dir — created, and empty by
/// construction: the pid+seq pair has never been issued before, so no prior content can
/// exist (this subsumes every legacy remove-then-create dance).
pub(crate) fn scratch_dir(label: &str) -> PathBuf {
    let d = scratch_name(label);
    std::fs::create_dir_all(&d).expect("scratch_dir: create");
    d
}
```

- Format: `wc-scratch-{pid}-{seq}-{label}`, label always last, extension rides in the label.
  (The existing `pub mod scratch` — the scratch-*buffer* feature — is unrelated; no name
  collision, both are module-scoped.)
- Counter: `AtomicU64` (decision 6) — three legacy helpers already used u64, widening is
  free, and it forecloses the wrap conversation permanently.
- The exact fn split (`scratch_name` private vs inlined) is the implementer's; the CONTRACT
  is: one shared static, one format, path-not-created vs dir-created-and-empty.

### 2.2 Uniqueness contract + invariant test (decision 6)

**Contract:** any interleaving of `scratch_path`/`scratch_dir` calls across threads of one
test process yields pairwise-distinct paths; every `scratch_dir` result exists and is empty
at return; every `scratch_path` result does not exist at return. Mechanism: per-process
`process::id()` + the single crate-global counter — one source of truth, so the cross-helper
counter-aliasing fragility the census flagged (`swap.rs`'s shared SEQ, `fsx.rs`'s split
statics) structurally cannot recur.

**Invariant test** (lives in `test_support.rs`'s own `mod tests`): ≥32 threads (the runner
width that surfaced H31 — cf. the harness floor guard, commit `4cd869b`) × 8 calls each,
alternating both fns; collect all returned paths into a set; assert set size == call count;
assert each `scratch_dir` result `read_dir`s empty and each `scratch_path` result
`!exists()`; remove created dirs at the end. Pid recycling against a prior crashed run's
litter is the one theoretical residue (astronomically unlikely; the legacy helpers already
accepted it; this test makes any occurrence observable).

**TDD red:** genuine. Write the invariant test first — it fails to compile (the fns do not
exist); a deliberately naive first body (label-only, no counter) fails the distinctness
assertion at 32 threads before the counter lands. Red → green is real on both axes.

### 2.3 `nix_privileged` fold

Replace its fixed-name `wc-priv-{pid}` remove/create with `let d = scratch_dir("priv");`
(the trailing cleanup `remove_dir_all`s stay). One-line fix of a latent H31-shaped hazard in
the seam's own file; its single caller (`file_browser.rs`, the unreadable-dir test) is
unaffected. Pin: the existing caller's suite.

### 2.4 Migration (decision 2) — delegation-body style

Each legacy helper's BODY becomes a one-line delegation; its ~150–160 call sites are
untouched. The duplication removed is the pid/counter/format LOGIC — a thin local wrapper
that only fixes the module label is the point, not a failure of it. Every legacy static and
its now-unused `use std::sync::atomic::…` import is deleted (an orphaned static or import is
a warning, and warnings are a gate).

| Legacy helper | Delegation body |
|---|---|
| `file.rs::scratch_path(label)` | `crate::test_support::scratch_path(&format!("file-{label}.txt"))` |
| `settings.rs::tempdir()` | `crate::test_support::scratch_dir("settings")` |
| `save.rs::scratch()` | `crate::test_support::scratch_path("bgsave.md")` |
| `config.rs::tempdir()` | `crate::test_support::scratch_dir("cfg")` |
| `config.rs::scratch_cfg_path(name)` | `crate::test_support::scratch_path(&format!("cfg-{name}.toml"))` |
| `file_browser_commit.rs::tmp(label)` | `crate::test_support::scratch_dir(&format!("commit-{label}"))` — the legacy `remove_dir_all` is dropped, subsumed by fresh-unique (§2.1) |
| `fsx.rs::private_dir(label)` | `crate::test_support::scratch_dir(&format!("fsx-{label}"))` — its "private per-test dir so a `.tmp` glob isn't polluted" doc purpose is preserved by uniqueness |
| `fsx.rs::unique_dir(label)` | same body as `private_dir` (same `mod tests`; the duplicate wrapper may be collapsed into one — implementer's call, either is conformant) |
| `state.rs::tmp()` | `crate::test_support::scratch_dir("state")` |
| `swap.rs::scratch()` | `crate::test_support::scratch_path("swap.md")` |
| `swap.rs::unique_dir(label)` | `crate::test_support::scratch_dir(&format!("h5-{label}"))` |
| `app.rs::quit_tmp(tag)` | `crate::test_support::scratch_path(&format!("quit-{tag}.md"))` |
| `jobs_apply.rs::quit_tmp(tag)` | `crate::test_support::scratch_path(&format!("c4-{tag}.md"))` |
| `session_restore.rs::scratch()` | `crate::test_support::scratch_path("sessmig.md")` |
| `plugin/load.rs::unique_plugin_dir(label)` | `crate::test_support::scratch_dir(&format!("plug-{label}"))` |

Notes: the two `quit_tmp`s' tag-first name ordering is dropped (cosmetic — no test reads the
path shape back); no test anywhere asserts on a helper's filename pattern (verified: the
only shape-adjacent assertion is `swap.rs`'s `starts_with(temp_dir())`, which the seam
trivially satisfies).

**Inline SEQ co-users** (§1.2) migrate in the SAME commit as their module's static deletion:
dir-shaped sites become `scratch_dir("<label>")` and drop their now-redundant
`create_dir_all` line; path-shaped sites become `scratch_path("<label>.md")`; the rekey pair
becomes two independent `scratch_path` calls (`"rekey-old.md"`, `"rekey-new.md"`).

**Pins, not reds:** every migration commit is behavior-preserving; the existing module
suites are the pin. The plan must not manufacture failing tests here.

---

## 3. Design — H33: the HOME seam

### 3.1 `pathx::expand_tilde` (decision 3) — one pure function, strict behavior preservation

New module `wordcartel/src/pathx.rs` (`pub mod pathx;` in `lib.rs` — the `-x` suffix follows
`fsx`/`panicx` house convention; `pub` visibility is also what keeps the additive commit
warning-free, exactly as the zero-caller `prompts::expand_path` compiles clean today):

```rust
/// Expand a tilde against an EXPLICIT home directory — the pure core of every `~` site.
///
/// - `"~"`        -> `home`, or the literal `"~"` when `home` is `None`.
/// - `"~/rest"`   -> `home/rest`, or the literal input when `home` is `None`.
/// - anything else -> `PathBuf::from(text)`, verbatim.
///
/// `home` is `dirs::home_dir()` at every production boundary and an injected temp dir in
/// tests — no caller of this function reads the process environment.
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

(Doc comment carries a runnable `# Examples` block per house style — public item.)

The four duplicated sites re-point at it with **exact semantic preservation** — each keeps
its own non-tilde policy, and the two sites that today do NOT expand bare `~` keep their
`starts_with("~/")` gate so bare-`~` behavior is unchanged:

| Site | Rewritten shape | `home` source |
|---|---|---|
| `config.rs::load_with_fs`, diagnostics dictionary | `cfg.diagnostics.dictionary = Some(expand_tilde(&s, dirs.home.as_deref()));` — full delegation; its legacy else-branch IS `PathBuf::from`, an exact match | the `PlatformDirs` param (§3.2) |
| `config.rs::load_with_fs`, theme `file` | `if s == "~" \|\| s.starts_with("~/") { expand_tilde(s, dirs.home.as_deref()) } else { /* legacy absolute-or-layer_dir-relative join, unchanged */ }` | the `PlatformDirs` param |
| `file_browser_commit.rs::resolve_field` | `if t.starts_with("~/") { return expand_tilde(t, home); }` — bare `~` still falls through to `dir.join` | new `home: Option<&Path>` parameter (§3.3) |
| `prompts.rs::expand_path` | `let expanded = if text.starts_with("~/") { expand_tilde(text, dirs::home_dir().as_deref()) } else { PathBuf::from(text) };` — bare `~` still falls through to the cwd join | resolved inline at its own boundary (it is itself the production entry; zero callers/tests today — §1.5) |

**Flagged for the record (not implemented):** unifying bare-`~` handling in
`resolve_field`/`expand_path` (so a user typing `~` in the destination field reaches home
instead of a literal `~` file) is a defensible UX improvement but a user-visible behavior
change — out of this effort; file separately if wanted.

**TDD red:** genuine. `expand_tilde`'s unit tests (in `pathx.rs`) are written first —
compile-red, then behavioral reds against an incremental body: `("~", Some)` → home;
`("~", None)` → `"~"`; `("~/a/b", Some)` → `home/a/b`; `("~/a", None)` → literal `"~/a"`;
`("plain", _)` → passthrough; `("a/~/b", _)` → passthrough (no mid-string expansion);
`("", _)` → empty passthrough. The `home == None` branches are coverage that today exists
NOWHERE (they were exactly the vacuous-skip holes).

### 3.2 `PlatformDirs` — the carrier (decision 4)

In `pathx.rs`, beside `expand_tilde`:

```rust
/// Platform directories resolved ONCE at a production boundary and passed down — the
/// injection carrier that keeps `dirs::*` reads out of pure code and out of tests.
pub struct PlatformDirs {
    pub home: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

impl PlatformDirs {
    /// Resolve from the real environment — production boundaries only; tests construct
    /// the struct literally with explicit paths.
    pub fn from_env() -> Self {
        PlatformDirs { home: dirs::home_dir(), config_dir: dirs::config_dir() }
    }
}
```

Owned `Option<PathBuf>` fields, public (the struct is a dumb carrier; a literal constructor
IS its test API — accessor ceremony would fight the point). Both fields have production
consumers, so no dead-code exposure:

- **`home`** — consumed by `load_with_fs` (§3.4).
- **`config_dir`** — consumed by `app.rs::run`: the existing `let xdg = dirs::config_dir();`
  becomes `let xdg = crate::pathx::PlatformDirs::from_env().config_dir;` (one line; the
  downstream `xdg.as_deref()` / `overrides_path` uses are untouched). This makes `from_env`
  the sanctioned resolution point at the app boundary — the same boundary that already
  passes `xdg` down into `config_layer_paths_with_fs` today.

### 3.3 `resolve_field` gains the home parameter; the `set_var` dies (decision 3)

- Signature: `pub(crate) fn resolve_field(dir: &Path, field: &str, home: Option<&Path>) -> PathBuf`.
- Its ONE production caller — inside `classify_destination_enter` (`file_browser_commit.rs`,
  the Enter decision table; `classify_destination_enter`'s own signature is UNCHANGED, so
  its internal caller `commit_destination_enter` region and external caller
  `file_browser::footer_target` are untouched) — resolves at the boundary:

  ```rust
  let home = dirs::home_dir();
  let resolved = resolve_field(dir, trimmed, home.as_deref());
  ```

  No `classify_destination_enter`-level test uses tilde input (verified: the only tilde
  literal in the file's tests is inside the `resolve_field` test), so no test path crosses
  this env read.
- Tests: `a_bare_relative_field_resolves_against_fb_dir_not_the_process_cwd`'s two
  `resolve_field` calls pass `None` (proving env independence), as does the absolute-path
  call in the tilde test; `absolute_and_home_relative_fields_are_honoured`
  is rewritten to pass `Some(&home)` where `home = tmp("resolve-home")` (post-§2.4: the seam
  underneath) — the assertion `got == home.join("notes.md")` stays **mandatory**, which was
  the entire point of the original `set_var`. The save/set/restore block (3
  `set_var`/`remove_var` calls) and both edition-2021 caveat comments are **deleted**; the
  test's explanatory comment is rewritten to describe injection ("the test OWNS the home
  answer by passing it") rather than mutation.
- **Edition-2024 posture, confirmed:** after this task, `grep -rn "set_var\|remove_var"`
  across the workspace returns nothing — the only env mutation is gone; the next edition
  bump has no `unsafe`-env work. Pin: green-on-arrival (mechanism swap; behavior already
  correct), stated honestly — the mandatory assertion is the pin.

### 3.4 `load_with_fs` carries the home (decision 4)

- Signature: `pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf],
  dirs: &crate::pathx::PlatformDirs) -> (Config, Vec<String>)`. Both tilde sites read
  `dirs.home.as_deref()` (§3.1). (Parameter naming note for the implementer: the `dirs`
  CRATE is also in scope here — either rename the param, e.g. `pdirs`, or use fully
  qualified `::dirs::home_dir()` where the crate is still meant; the compiler forces the
  choice, the spec just forewarns it.)
- `pub fn load(paths)` keeps its signature — it is already documented as "the `RealFs`
  wrapper — its `*_with_fs` seam is what injected callers use" (its `fs-chokepoint-allow (w)`
  comment), and the carrier extends that exact role:
  `load_with_fs(&crate::fsx::RealFs, paths, &crate::pathx::PlatformDirs::from_env())`.
  Consequently the ~30 test callers of `load(…)` and all five production `config::load`
  call sites (`app.rs` ×2, `e2e.rs` ×2, `plugin/reload.rs` ×1) are untouched.
- The one existing direct `load_with_fs` caller outside `load` — the config-cap test in
  `config.rs` (`load_with_fs(&crate::fsx::RealFs, std::slice::from_ref(&p))`) — adds
  `&PlatformDirs { home: None, config_dir: None }` (its assertion is size-cap behavior; env
  irrelevance made explicit).

### 3.5 The oracle tests go unconditional (decisions 4, 5)

End state: **zero tests read the process environment as an assertion oracle**, and the two
formerly vacuous-skip assertions become mandatory.

1. `config.rs::dictionary_tilde_is_expanded` — switches from `load(&[p])` to
   `load_with_fs(&crate::fsx::RealFs, &[p], &dirs)` with
   `dirs = PlatformDirs { home: Some(scratch_dir("cfg-home")), config_dir: None }`; the
   `if let Some(home) = dirs::home_dir()` guard is deleted and the
   `<home>/foo/dict.txt` equality asserts **unconditionally** against the injected home.
   The `warns.is_empty()` and no-literal-`~` backstop assertions stay.
2. `config.rs::dictionary_bare_tilde_expands_to_home` — same switch;
   `assert_eq!(cfg.diagnostics.dictionary, Some(injected_home))` **unconditionally**.
3. `config.rs::diagnostics_default_dictionary_is_not_none` (decision 5) — extract the pure
   default beside `DiagnosticsConfig`:

   ```rust
   /// `<config_dir>/wordcartel/dictionary.txt`, or None when the platform has no config dir.
   pub fn default_dictionary_path(config_dir: Option<&Path>) -> Option<PathBuf> {
       config_dir.map(|d| d.join("wordcartel").join("dictionary.txt"))
   }
   ```

   `impl Default for DiagnosticsConfig` delegates:
   `dictionary: default_dictionary_path(dirs::config_dir().as_deref())` (the `Default` trait
   has no injection point by nature — this ONE production `dirs::config_dir()` read remains,
   documented as such in its Fix-A7 comment). The test is rewritten to call
   `default_dictionary_path(Some(&explicit_dir))` and assert the joined path
   **unconditionally**, plus `default_dictionary_path(None) == None`; the
   `if let`/else-comment scaffolding is deleted. Optionally one thin conditional smoke
   assertion on `DiagnosticsConfig::default()` itself may remain to pin the delegation
   wiring — the plan decides; the unconditional pure-fn test is the required part.
4. The `resolve_field` test — §3.3.

**Pins, not reds — flagged honestly:** all four rewrites are green-on-arrival (the
underlying behavior is already correct; what changes is the oracle's source). The plan must
label them PINS. The genuinely new coverage in H33 is `expand_tilde`'s unit suite (§3.1,
real reds, including the never-before-tested `home == None` branches) and
`default_dictionary_path(None) == None`.

### 3.6 Remaining `dirs::*` reads after H33 (the honest census)

Production reads that REMAIN, each at a deliberate boundary: `pathx::PlatformDirs::from_env`
(the sanctioned resolution point), `DiagnosticsConfig::default` via
`default_dictionary_path` (Default has no injection point), `classify_destination_enter`'s
`resolve_field` boundary, `prompts::expand_path`'s internal boundary, and
`swap::state_dir`'s `#[cfg(not(test))]` arm (already test-diverted). No TEST reaches any of
them as an oracle. H33 does not chase further centralization — config-class reads are small
and deliberate (the H7 blast-radius stance).

---

## 4. Task decomposition

Ordered so every task leaves `cargo test --workspace` green and the touched crates
build/clippy warning-free — no intermediate red across commits. (The plan slices within
these; boundaries below are the seams.)

- **T1 — the scratch seam.** `scratch_path`/`scratch_dir` + `SCRATCH_SEQ` in
  `test_support.rs`; the ≥32-thread uniqueness-invariant test (TDD red per §2.2); the
  `nix_privileged` fold (§2.3). Purely additive apart from `nix_privileged`; green.
- **T2 — migrate the self-contained helpers.** The 10 helpers whose static has no other
  user (#2–6, 8, 9, 13–15 of §1.1 — module-level sole-user statics included): delegation
  bodies per §2.4, statics + dead imports deleted. Pin: existing suites. Green.
- **T3 — migrate the shared-static modules.** `file.rs`, `app.rs`, `swap.rs` (helper
  delegations + the 11 inline SEQ co-users of §1.2, statics deleted) and
  `fsx.rs::private_dir` (sole-user module static). Pin: existing suites. Green. — After T3,
  zero legacy scratch statics remain: `grep -rnE "static [A-Z_]*SEQ" wordcartel/src` returns
  exactly three survivors, all production and unrelated — `editor.rs::DocumentId::mint`'s
  fn-local `SEQ`, `fsx.rs`'s `TEMP_SEQ` (atomic-write temp naming), and `clipboard.rs`'s
  `PASTE_SEQ` (paste-id sequence).
- **T4 — `pathx` module.** `expand_tilde` + its unit suite (TDD reds per §3.1) and
  `PlatformDirs`/`from_env` (consumed in T5; `pub` visibility keeps this commit
  warning-free, §3.1). Green.
- **T5 — thread the carrier.** `load_with_fs` signature + both tilde sites → `expand_tilde`
  (§3.1, §3.4); `load` builds `from_env`; the config-cap test adds the empty carrier;
  `app.rs` `xdg` line swaps to the carrier (§3.2). Pin: the whole config suite. Green.
- **T6 — oracle tests unconditional.** The two tilde oracle tests re-oracled onto an
  injected home (§3.5.1–2, PINS); `default_dictionary_path` extraction + its unconditional
  test (§3.5.3; PIN + the new `None` case). Green.
- **T7 — `resolve_field` + the `set_var` deletion.** Home parameter, boundary resolution in
  `classify_destination_enter`, `expand_path` re-point, test rewrite, mutation + caveat
  comments deleted, edition-2024 confirmation grep (§3.3, §3.1). Pin: the mandatory
  tilde assertion. Green.

Dependency edges: T2/T3 need T1; T5–T7 need T4; T6 needs T5. T2 and T4 are independent of
each other (the plan may parallelize implementers there if it wishes — the ledger rules
apply as usual).

## 5. Gates (restated for the plan)

`cargo test --workspace` green after EVERY task; `cargo build` + `cargo test --no-run`
warning-free for `wordcartel`; workspace clippy clean (`cargo clippy --workspace
--all-targets`); no `cargo fmt`; house style by hand (em-dashes in prose comments, hand
wrapping, imports grouped by hand). `module_budgets` and `fs_chokepoint` verified
non-implicated (§1.3) — no budget bump, no new exemption marker. PTY smoke suite run once
pre-merge, one-line summary quoted verbatim (advisory). At ship: H32 + H33 → `shipped` in
`backlog.toml`, prose to `docs/backlog-archive.md`, `scripts/backlog bless` (H36 remains
open, `depends_on` satisfied). New durable guardrail added by this effort: the T1
uniqueness-invariant test.

## History

- 2026-07-25 — authored from the ruled forks (`scratchpad/h32h33/h32h33-forks.md`); all six
  decisions locked by the human as recorded in §0/§2/§3.
