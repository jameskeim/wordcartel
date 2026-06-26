# Wordcartel Effort 5f — Harper Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Background grammar + spell diagnostics via in-process Harper — underlined markers (two tiers), live-debounced re-check off the input loop, a quick-fix overlay to accept suggestions / ignore / add-to-dictionary, and next/prev-diagnostic motions.

**Architecture:** A new IO-free `wordcartel-core::diagnostics` module wraps `harper-core` (`check(text,&opts) -> Vec<Diagnostic>`, pure, unit-tested). The shell debounces edits (riding the existing `recv_timeout` loop deadline), runs the check on a **spawned worker thread** (the `dispatch_filter` `msg_tx` pattern — NOT the Executor), surfaces a **version-gated** `Msg::DiagnosticsDone`, stores diagnostics per-buffer (valid only when `computed_version == buffer.version`), projects underline markers through `ColMap` in render (generalizing the 5e search-highlight fork), and owns the quick-fix overlay + dictionary file I/O.

**Tech Stack:** Rust, `harper-core` (new core dep), `ropey`, `ratatui 0.29` (`Style::underline_color`), `crossterm 0.28`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-06-26-wordcartel-05f-harper-diagnostics-design.md` — authoritative.
- **Functional core:** `wordcartel-core` is IO/thread-free and `#![forbid(unsafe_code)]`. `harper-core` may use unsafe internally; **our** core code may not. **Core does NO filesystem IO** — Harper's main dictionary must be embeddable (static) or **shell-injected** via `CheckOpts`; the personal dictionary is shell-loaded. New core dep must be **MIT/Apache** (confirm at the gate).
- **Diagnostics are MARKERS, never buffer edits.** The only buffer mutation is an explicit user "accept suggestion", applied as a normal undoable `ChangeSet` edit.
- **Async path:** the `dispatch_filter` **spawned-thread + `msg_tx`** pattern (filter.rs:322). Dedicated `Msg::DiagnosticsDone { buffer_id, version, diagnostics }`, **version-gated in `reduce`** (applied only if `version == buffer.version`, else discarded), with a per-buffer `in_flight_version` guard. **Not** the `Executor`/`JobResult`/`Msg::JobDone` path.
- **Debounce rides the existing loop:** fold the active buffer's `recheck_due_at` into the loop's deadline `min()` (app.rs:1194–1232, computed from `clock.now_ms()`); timeout produces `Msg::Tick`. Extract the deadline math into a **pure** helper so it is testable (the live loop is not exercised by `reduce()` tests).
- **Staleness = hide-then-replace:** markers paint only when `computed_version == buffer.version`; on any edit/undo/redo (version bumps monotonically, editor.rs:82/100/106) markers are **hidden** until the next debounced re-check. **No offset remapping.**
- **Render:** generalize the 5e `placed`-path fork (gated on search today, render.rs ~272/288) to fire when **search OR diagnostics** apply; project diagnostic ranges through `ColMap.placed[].src`, **viewport-bounded** (`partition_point`). Inactive/empty/stale = **true no-op** (existing render tests unchanged).
- **Overlay XOR (not centralized):** every `open_*` clears `diag`; `open_diag` clears all siblings + `pending_keys` + `pending_mark`; clear `diag` at menu/mouse-click-outside/save-reload sites too. The diag reduce branch lets **non-key** messages fall through (5e lesson — don't starve `DiagnosticsDone`/`FilterDone`).
- **Keys** bound in the **CUA preset** in keymap.rs (production), mirrored in `input::key_to_command_id` (test-only). `Ctrl+.` / `F8` / `Shift+F8` are free (confirmed).
- **Default-on** diagnostics, but existing `reduce()` tests stay deterministic: dispatch only when enabled AND debounce elapsed; render/dispatch are no-ops when the store is empty.
- Commit trailers on every commit:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```
- `cargo test -p wordcartel-core` / `cargo test -p wordcartel`; zero build warnings. Baseline at branch start: 126 core + 313 shell lib.

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `wordcartel-core/Cargo.toml` | Modify | add `harper-core` dep |
| `wordcartel-core/src/lib.rs` | Modify | `pub mod diagnostics;` |
| `wordcartel-core/src/diagnostics.rs` | **Create** | `Diagnostic`/`DiagnosticKind`/`CheckOpts`/`check`; Harper wrap. Pure. |
| `wordcartel/src/config.rs` | Modify | `[diagnostics]` → `RawDiagnostics` + `DiagnosticsConfig` + validation |
| `wordcartel/src/editor.rs` | Modify | `Buffer.diagnostics: DiagStore`; `Editor.diag: Option<DiagOverlay>`; `open_diag`; every `open_*` clears `diag` |
| `wordcartel/src/diagnostics_run.rs` | **Create** | `DiagStore`; pure `next_deadline`/`diag_due`; `dispatch_diagnostics`; `apply_diagnostics_done`; dictionary file load/append; `CheckOpts` assembly |
| `wordcartel/src/diag_overlay.rs` | **Create** | `DiagOverlay` quick-fix picker state |
| `wordcartel/src/app.rs` | Modify | `Msg::DiagnosticsDone`; reduce() Tick-dispatch + version-gated apply + debounce arming; diag overlay interception; clear `diag` on buffer-swap |
| `wordcartel/src/render.rs` | Modify | generalize placed-path fork; diagnostic underline layer; overlay paint |
| `wordcartel/src/registry.rs` | Modify | commands: `quick_fix`, `diag_next`, `diag_prev`, `recheck_diagnostics` |
| `wordcartel/src/input.rs` / `keymap.rs` | Modify | `Ctrl+.` / `F8` / `Shift+F8` binds |
| `wordcartel/src/mouse.rs` / `save.rs` | Modify | clear `editor.diag` at click-outside / reload sites |

---

## Task 1: Core diagnostics module + Harper build gate

**This is the risk gate — prove Harper builds and runs purely before any feature code.**

**Files:**
- Modify: `wordcartel-core/Cargo.toml`, `wordcartel-core/src/lib.rs`
- Create: `wordcartel-core/src/diagnostics.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum DiagnosticKind { Spelling, Grammar }
  pub struct Diagnostic { pub range: std::ops::Range<usize>, pub kind: DiagnosticKind, pub message: String, pub suggestions: Vec<String> }
  pub struct CheckOpts<'a> { pub grammar: bool, pub ignore_words: &'a std::collections::HashSet<String> }
  pub fn check(text: &str, opts: &CheckOpts) -> Vec<Diagnostic>; // sorted by range.start
  ```

- [ ] **Step 1: Add the dependency**

In `wordcartel-core/Cargo.toml` under `[dependencies]`:
```toml
harper-core = "0.x"   # resolve the actual latest at the gate
```

- [ ] **Step 2: Build gate — prove it compiles and is pure**

Run: `cargo build -p wordcartel-core`
Expected: PASS. Then verify, and **STOP + report BLOCKED** if any fails:
- (a) compiles against the workspace (no dep conflicts);
- (b) `#![forbid(unsafe_code)]` still holds (our code adds no `unsafe`);
- (c) license is MIT/Apache (`cargo tree`/crate metadata);
- (d) **Harper's main dictionary is usable WITHOUT core filesystem IO** — it is embedded/static in the crate, OR a checker/dictionary handle can be constructed in-memory. If Harper *requires* loading a dictionary file at runtime, STOP and report: the dictionary load must move to the shell and be injected via `CheckOpts` (escalate for the signature change);
- (e) Harper exposes lint spans (char or byte), a category/kind, a message, and suggestions, and char↔byte conversion is feasible.
Record the resolved version, size impact (`ls -la target/.../libwordcartel_core*` or `cargo bloat` if available), and the real API shape in the report.

- [ ] **Step 3: Declare the module**

In `wordcartel-core/src/lib.rs`, alongside the other `pub mod` lines:
```rust
pub mod diagnostics;
```

- [ ] **Step 4: Write the failing tests**

Create `wordcartel-core/src/diagnostics.rs` ending with:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn opts<'a>(grammar: bool, ignore: &'a HashSet<String>) -> CheckOpts<'a> {
        CheckOpts { grammar, ignore_words: ignore }
    }

    #[test]
    fn flags_a_misspelling_as_spelling_with_byte_range() {
        let ignore = HashSet::new();
        let ds = check("I love teh cat.", &opts(true, &ignore));
        let spell: Vec<_> = ds.iter().filter(|d| d.kind == DiagnosticKind::Spelling).collect();
        assert!(!spell.is_empty(), "expected a spelling diagnostic for 'teh'");
        let d = spell[0];
        assert_eq!(&"I love teh cat."[d.range.clone()], "teh", "range must cover the misspelled word");
        assert!(d.suggestions.iter().any(|s| s == "the"), "expected 'the' among suggestions, got {:?}", d.suggestions);
    }

    #[test]
    fn repeated_word_is_grammar_and_suppressed_when_grammar_off() {
        let ignore = HashSet::new();
        let on = check("the the cat", &opts(true, &ignore));
        assert!(on.iter().any(|d| d.kind == DiagnosticKind::Grammar), "repeated 'the' should be a Grammar diagnostic");
        let off = check("the the cat", &opts(false, &ignore));
        assert!(off.iter().all(|d| d.kind != DiagnosticKind::Grammar), "grammar=false must suppress Grammar diagnostics");
    }

    #[test]
    fn ignore_words_drops_the_diagnostic() {
        let mut ignore = HashSet::new();
        ignore.insert("teh".to_string());
        let ds = check("teh cat", &opts(true, &ignore));
        assert!(ds.iter().all(|d| &"teh cat"[d.range.clone()] != "teh"), "ignored word must not be flagged");
    }

    #[test]
    fn multibyte_offsets_are_byte_accurate() {
        // 'é' is 2 bytes; the misspelling after it must have correct BYTE offsets.
        let ignore = HashSet::new();
        let text = "café teh";
        let ds = check(text, &opts(true, &ignore));
        let d = ds.iter().find(|d| text.get(d.range.clone()) == Some("teh")).expect("byte-accurate 'teh'");
        assert_eq!(d.range.start, "café ".len()); // 6 bytes
    }

    #[test]
    fn deterministic_and_sorted() {
        let ignore = HashSet::new();
        let a = check("teh teh", &opts(true, &ignore));
        let b = check("teh teh", &opts(true, &ignore));
        assert_eq!(a, b, "check must be deterministic");
        assert!(a.windows(2).all(|w| w[0].range.start <= w[1].range.start), "sorted by range.start");
    }
}
```

- [ ] **Step 5: Run to verify they fail**

Run: `cargo test -p wordcartel-core diagnostics::`
Expected: FAIL (compile errors — types/`check` not defined).

- [ ] **Step 6: Implement `diagnostics.rs`**

```rust
//! In-document grammar/spell diagnostics (spec §3.1). Wraps `harper-core`.
//! PURE: no IO, no threads, no global mutable state. Deterministic per
//! (text, opts). The shell injects the personal dictionary via
//! `CheckOpts.ignore_words`; the main dictionary is embedded by Harper (Task-1
//! gate confirms no core filesystem IO).
use std::collections::HashSet;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagnosticKind { Spelling, Grammar }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub range: Range<usize>,     // byte range into `text`
    pub kind: DiagnosticKind,
    pub message: String,
    pub suggestions: Vec<String>,
}

pub struct CheckOpts<'a> {
    pub grammar: bool,
    pub ignore_words: &'a HashSet<String>,
}

/// Run Harper over `text`, returning diagnostics sorted ascending by
/// `range.start`. Spelling lints → DiagnosticKind::Spelling; the curated
/// grammar/style set → DiagnosticKind::Grammar (suppressed when !opts.grammar).
/// Words in `ignore_words` (case-insensitive on the flagged surface form) are
/// dropped. Harper char-spans are converted to BYTE ranges.
pub fn check(text: &str, opts: &CheckOpts) -> Vec<Diagnostic> {
    // IMPLEMENTATION NOTE (resolved at Task-1 gate): construct Harper's
    // Document + run its linters, iterate lints. For each lint:
    //   - map its char span → byte range via `text` char_indices (or Harper's
    //     own byte API if exposed);
    //   - classify: Harper's spelling lint → Spelling; the curated grammar set
    //     → Grammar; drop lints outside the enabled set;
    //   - if Spelling and the flagged surface form (case-insensitive) is in
    //     ignore_words, skip;
    //   - collect message + suggestion replacement strings.
    // Then if !opts.grammar, drop all Grammar diagnostics. Sort by range.start.
    // Keep this function pure: no fs, no statics beyond Harper's embedded data.
    let mut out: Vec<Diagnostic> = harper_lints(text)   // see helper below
        .into_iter()
        .filter_map(|lint| {
            let kind = classify(&lint)?;                  // None → not in enabled set
            if !opts.grammar && kind == DiagnosticKind::Grammar { return None; }
            let range = char_span_to_bytes(text, lint.span);
            if kind == DiagnosticKind::Spelling {
                let surface = text.get(range.clone()).unwrap_or("").to_lowercase();
                if opts.ignore_words.iter().any(|w| w.to_lowercase() == surface) { return None; }
            }
            Some(Diagnostic { range, kind, message: lint.message, suggestions: lint.suggestions })
        })
        .collect();
    out.sort_by_key(|d| d.range.start);
    out
}
// `harper_lints`, `classify`, `char_span_to_bytes`, and the `Lint` shim struct
// are thin adapters over the resolved harper-core API — implement them here.
```
> Implementer note: the adapters (`harper_lints`/`classify`/`char_span_to_bytes`)
> are the only Harper-version-specific code. Keep the `check` signature and ALL
> tests fixed; adapt only the adapter bodies. The curated grammar allow-list
> (which Harper linters map to `Grammar`) is enumerated here from Harper's real
> linter catalog discovered at the gate — start with a small high-signal set
> (e.g. repeated words, obvious capitalization/agreement); the shell config
> (Task 2) can pare it. If Harper requires a runtime dictionary file, STOP per
> Step 2(d).

- [ ] **Step 7: Run to verify they pass**

Run: `cargo test -p wordcartel-core diagnostics::`
Expected: PASS (5 tests). `cargo build -p wordcartel-core 2>&1 | grep -i warning` empty.

- [ ] **Step 8: Commit**

```bash
git add wordcartel-core/Cargo.toml wordcartel-core/src/lib.rs wordcartel-core/src/diagnostics.rs
git commit -F <commit-msg-file>   # subject: feat(core): diagnostics — Harper wrap (check → Diagnostic), pure + unit-tested
```

---

## Task 2: `[diagnostics]` config section

**Files:**
- Modify: `wordcartel/src/config.rs`

**Interfaces:**
- Produces: `DiagnosticsConfig { enabled: bool, grammar: bool, debounce_ms: u64, dictionary: Option<PathBuf>, linters: Option<Vec<String>> }` on `Config`.

- [ ] **Step 1: Write the failing test**

In `config.rs` tests:
```rust
#[test]
fn diagnostics_config_defaults_and_validation() {
    // default: enabled, grammar on, debounce 400
    let (cfg, _warns) = load(&[]);
    assert!(cfg.diagnostics.enabled);
    assert!(cfg.diagnostics.grammar);
    assert_eq!(cfg.diagnostics.debounce_ms, 400);
}

#[test]
fn diagnostics_debounce_is_clamped_with_warning() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("c.toml");
    std::fs::write(&p, "[diagnostics]\ndebounce_ms = 5\n").unwrap();
    let (cfg, warns) = load(&[p]);
    assert_eq!(cfg.diagnostics.debounce_ms, 100, "debounce_ms clamped to floor 100");
    assert!(warns.iter().any(|w| w.contains("debounce_ms")), "clamp warns");
}
```
(Use the existing config-test helpers; mirror how `RawView` tests are written.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel diagnostics_config`
Expected: FAIL (`cfg.diagnostics` field absent).

- [ ] **Step 3: Implement**

Add to `RawConfig` (config.rs:111) a field `diagnostics: RawDiagnostics`, and:
```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawDiagnostics {
    enabled: Option<bool>,
    grammar: Option<bool>,
    debounce_ms: Option<u64>,
    dictionary: Option<String>,
    linters: Option<Vec<String>>,
}
```
Add the typed config:
```rust
#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub grammar: bool,
    pub debounce_ms: u64,
    pub dictionary: Option<std::path::PathBuf>,
    pub linters: Option<Vec<String>>,
}
impl Default for DiagnosticsConfig {
    fn default() -> Self { DiagnosticsConfig { enabled: true, grammar: true, debounce_ms: 400, dictionary: None, linters: None } }
}
```
Add `pub diagnostics: DiagnosticsConfig` to `Config`. In `load()` build it from `RawDiagnostics`, pushing to `warns`:
```rust
let mut d = DiagnosticsConfig::default();
if let Some(v) = raw.diagnostics.enabled { d.enabled = v; }
if let Some(v) = raw.diagnostics.grammar { d.grammar = v; }
if let Some(v) = raw.diagnostics.debounce_ms {
    if v < 100 { warns.push(format!("config: diagnostics.debounce_ms {v} below floor 100; clamped")); d.debounce_ms = 100; }
    else { d.debounce_ms = v; }
}
d.dictionary = raw.diagnostics.dictionary.map(|s| crate::config::expand_path(&s)); // reuse existing path expansion if present, else PathBuf::from
d.linters = raw.diagnostics.linters;
// unknown linter names are validated against the core catalog later (Task 4 assembly) — warn there.
cfg.diagnostics = d;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel diagnostics_config`
Expected: PASS. Full `cargo test -p wordcartel --lib` green.

- [ ] **Step 5: Commit** (subject: `feat(config): [diagnostics] section + validation (clamp debounce, defaults)`)

---

## Task 3: `DiagStore` + per-buffer wiring + pure debounce helpers

**Files:**
- Create: `wordcartel/src/diagnostics_run.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod diagnostics_run;`), `wordcartel/src/editor.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct DiagStore {
      pub diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
      pub computed_version: u64,
      pub recheck_due_at: Option<u64>,
      pub in_flight_version: Option<u64>,
  }
  impl DiagStore { pub fn new() -> Self; pub fn valid_for(&self, version: u64) -> bool; pub fn arm(&mut self, now: u64, debounce_ms: u64); }
  /// Pure: the smallest deadline among the terms (None terms ignored).
  pub fn next_deadline(terms: &[Option<u64>]) -> Option<u64>;
  /// Pure: is a re-check due (armed, reached, and not already in flight for `version`)?
  pub fn diag_due(store: &DiagStore, now: u64, version: u64) -> bool;
  ```
- `Buffer` gains `pub diagnostics: DiagStore`.

- [ ] **Step 1: Write the failing tests**

Create `wordcartel/src/diagnostics_run.rs` ending with:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_deadline_is_min_ignoring_none() {
        assert_eq!(next_deadline(&[None, Some(50), None, Some(20), Some(99)]), Some(20));
        assert_eq!(next_deadline(&[None, None]), None);
    }

    #[test]
    fn arm_sets_due_and_valid_for_tracks_version() {
        let mut s = DiagStore::new();
        assert!(!s.valid_for(0)); // empty store: computed_version default != a fresh version? see new()
        s.arm(1000, 400);
        assert_eq!(s.recheck_due_at, Some(1400));
    }

    #[test]
    fn diag_due_requires_armed_reached_and_not_in_flight() {
        let mut s = DiagStore::new();
        s.arm(1000, 400);
        assert!(!diag_due(&s, 1399, 7), "not yet due");
        assert!(diag_due(&s, 1400, 7), "due at deadline");
        s.in_flight_version = Some(7);
        assert!(!diag_due(&s, 1500, 7), "already in flight for this version");
    }

    #[test]
    fn valid_for_only_when_computed_version_matches() {
        let mut s = DiagStore::new();
        s.computed_version = 5;
        s.diagnostics.push(wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            message: "x".into(), suggestions: vec![] });
        assert!(s.valid_for(5));
        assert!(!s.valid_for(6)); // edited since → hidden
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel diagnostics_run::`
Expected: FAIL (not defined).

- [ ] **Step 3: Implement the store + pure helpers**

```rust
//! Diagnostics runtime (shell): per-buffer store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::Diagnostic;

#[derive(Debug, Default, Clone)]
pub struct DiagStore {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,
    pub recheck_due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
}
impl DiagStore {
    pub fn new() -> Self { DiagStore::default() }
    /// Markers are paintable only when computed against the current version
    /// AND there is something to paint.
    pub fn valid_for(&self, version: u64) -> bool {
        !self.diagnostics.is_empty() && self.computed_version == version
    }
    /// Arm a re-check `debounce_ms` from `now`.
    pub fn arm(&mut self, now: u64, debounce_ms: u64) {
        self.recheck_due_at = Some(now.saturating_add(debounce_ms));
    }
}

/// Smallest of the deadline terms; None terms ignored.
pub fn next_deadline(terms: &[Option<u64>]) -> Option<u64> {
    terms.iter().flatten().copied().min()
}

/// A re-check is due if armed, the time has been reached, and no check is
/// already in flight for this exact version.
pub fn diag_due(store: &DiagStore, now: u64, version: u64) -> bool {
    matches!(store.recheck_due_at, Some(t) if now >= t)
        && store.in_flight_version != Some(version)
}
```

- [ ] **Step 4: Wire `DiagStore` onto `Buffer`**

In `editor.rs`: add `pub diagnostics: crate::diagnostics_run::DiagStore,` to `Buffer`; initialize `diagnostics: crate::diagnostics_run::DiagStore::new(),` in the buffer constructor. Add `pub mod diagnostics_run;` to `lib.rs`.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p wordcartel diagnostics_run::`
Expected: PASS. Full `cargo test -p wordcartel --lib` green (new field, no behavior change).

- [ ] **Step 6: Commit** (subject: `feat(diag): DiagStore + pure debounce helpers (next_deadline/diag_due) + per-buffer wiring`)

---

## Task 4: Worker dispatch + `Msg::DiagnosticsDone` + version-gated apply + debounce in the loop

**Files:**
- Modify: `wordcartel/src/app.rs`, `wordcartel/src/diagnostics_run.rs`, `wordcartel/src/config.rs` (dictionary load helper if needed)

**Interfaces:**
- Consumes: `DiagStore`, `next_deadline`, `diag_due` (Task 3); `core::diagnostics::{check, CheckOpts}` (Task 1); `DiagnosticsConfig` (Task 2); the `dispatch_filter` template (filter.rs:322); the FilterDone reduce-gating template (app.rs:747).
- Produces: `Msg::DiagnosticsDone { buffer_id, version, diagnostics }`; `dispatch_diagnostics(...)`; `apply_diagnostics_done(...)`; debounce arming + Tick-dispatch in `reduce`.

- [ ] **Step 1: Write the failing tests**

In `app.rs` tests:
```rust
#[test]
fn diagnostics_done_applies_only_for_current_version() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
    let bid = e.active().id;
    let v = e.active().document.version;
    let diag = vec![wordcartel_core::diagnostics::Diagnostic {
        range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
        message: "misspelled".into(), suggestions: vec!["the".into()] }];
    // current version → stored
    apply_diagnostics_done(&mut e, bid, v, diag.clone());
    assert_eq!(e.active().diagnostics.diagnostics.len(), 1);
    assert_eq!(e.active().diagnostics.computed_version, v);
    // stale version → discarded
    apply_diagnostics_done(&mut e, bid, v.wrapping_sub(1), diag);
    assert_eq!(e.active().diagnostics.diagnostics.len(), 1, "stale result must not overwrite");
}

#[test]
fn tick_dispatches_a_due_check_once() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    let mut e = Editor::new_from_text("teh\n", None, (80, 24));
    e.active_mut().diagnostics.arm(0, 400); // due at 400
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(500); // past due
    // a Tick at now=500 with diagnostics enabled dispatches one check
    reduce(Msg::Tick, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert_eq!(e.active().diagnostics.in_flight_version, Some(e.active().document.version));
    // the spawned worker sends a DiagnosticsDone
    match rx.recv().unwrap() {
        Msg::DiagnosticsDone { diagnostics, .. } => assert!(diagnostics.iter().any(|d| d.kind == wordcartel_core::diagnostics::DiagnosticKind::Spelling)),
        o => panic!("expected DiagnosticsDone, got {o:?}"),
    }
}
```
> Note: `tick_dispatches_a_due_check_once` requires diagnostics enabled in the test editor's config and a controllable clock `TestClock`. If `Editor::new_from_text` does not carry a config, seed `e` with a `DiagnosticsConfig::default()` the same way other tests seed `view_opts`/`mouse_capture`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel diagnostics_done_applies tick_dispatches`
Expected: FAIL (Msg variant + fns absent).

- [ ] **Step 3: Add the Msg variant**

In `app.rs` `enum Msg`, beside `TransformDone`:
```rust
DiagnosticsDone {
    buffer_id: crate::editor::BufferId,
    version: u64,
    diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
},
```
Add a `Debug` arm for it (mirror `TransformDone`'s).

- [ ] **Step 4: Implement dispatch + apply (in diagnostics_run.rs)**

```rust
use crate::editor::{BufferId, Editor};

/// Spawn a worker thread that runs Harper and sends Msg::DiagnosticsDone.
/// Mirrors filter::dispatch_filter (spawn + msg_tx). Sets in_flight_version.
pub fn dispatch_diagnostics(
    editor: &mut Editor,
    cfg: &crate::config::DiagnosticsConfig,
    ignore_words: std::sync::Arc<std::collections::HashSet<String>>,
    msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    let text = b.document.buffer.snapshot().to_string();
    let grammar = cfg.grammar;
    editor.active_mut().diagnostics.in_flight_version = Some(version);
    editor.active_mut().diagnostics.recheck_due_at = None; // consumed
    std::thread::spawn(move || {
        let opts = wordcartel_core::diagnostics::CheckOpts { grammar, ignore_words: &ignore_words };
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wordcartel_core::diagnostics::check(&text, &opts)
        })).unwrap_or_default(); // Harper panic → no diagnostics, never crash the loop (spec §8)
        let _ = msg_tx.send(crate::app::Msg::DiagnosticsDone { buffer_id, version, diagnostics });
    });
}

/// Version-gated apply: store only if `version` is still current for `buffer_id`.
pub fn apply_diagnostics_done(
    editor: &mut Editor,
    buffer_id: BufferId,
    version: u64,
    diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
) {
    if let Some(b) = editor.buffer_by_id_mut(buffer_id) {
        if b.document.version == version {
            b.diagnostics.diagnostics = diagnostics;
            b.diagnostics.computed_version = version;
        }
        // clear in_flight for this version regardless (the check completed)
        if b.diagnostics.in_flight_version == Some(version) {
            b.diagnostics.in_flight_version = None;
        }
    }
}
```
> `buffer_by_id_mut` exists (used by FilterDone merge); if the helper name differs, use the same lookup `apply_filter_done` uses.

- [ ] **Step 5: Wire reduce() — arm on edit, dispatch on Tick, apply DiagnosticsDone**

In `reduce`:
- Add the apply arm: `Msg::DiagnosticsDone { buffer_id, version, diagnostics } => crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics),`
- **Arm on edit:** after any handler that bumps `buffer.version` (compare the pre/post version, the loop already tracks `let before = editor.active().document.version;` per app.rs), if diagnostics enabled and the version changed: `editor.active_mut().diagnostics.arm(clock.now_ms(), cfg.diagnostics.debounce_ms);`
- **Dispatch on Tick (and any wake):** at the end of `reduce` (or in the `Msg::Tick` arm), if `cfg.diagnostics.enabled` and `diag_due(&store, clock.now_ms(), version)`: assemble `ignore_words` (Arc of personal-dict ∪ session ignores) and call `dispatch_diagnostics(editor, &cfg.diagnostics, ignore, msg_tx.clone())`.
- The `cfg` must be reachable in `reduce` — thread the `DiagnosticsConfig` (and the loaded personal dictionary + session ignore set) through the editor (e.g. `editor.diag_cfg: DiagnosticsConfig`, `editor.dictionary: HashSet<String>`, `editor.session_ignores: HashSet<String>`), seeded at startup like `view_opts`. Add those fields in this task.

- [ ] **Step 6: Extract the loop deadline helper (run())**

In `run()` (app.rs ~1215), replace the nested `min()` deadline composition with a call to `crate::diagnostics_run::next_deadline(&[swap_deadline, sq_deadline, sb_deadline, editor.active().diagnostics.recheck_due_at])`. (This both adds the debounce term and makes the math the pure, tested helper.)

- [ ] **Step 7: Run to verify they pass**

Run: `cargo test -p wordcartel diagnostics_done_applies tick_dispatches`
Expected: PASS. Full `cargo test -p wordcartel --lib` green (existing tests unaffected — diagnostics only dispatch when enabled AND due; a fresh editor without an armed/elapsed debounce never trips).

- [ ] **Step 8: Commit** (subject: `feat(diag): worker dispatch + Msg::DiagnosticsDone (version-gated) + debounce arming/Tick-dispatch + loop deadline helper`)

---

## Task 5: Render — diagnostic underline layer (generalize the placed-path)

**Files:**
- Modify: `wordcartel/src/render.rs`

**Interfaces:**
- Consumes: `Buffer.diagnostics: DiagStore`, `DiagStore::valid_for`; `ColMap.placed[]`; the 5e `placed`-path builder + `partition_point` windowing.

- [ ] **Step 1: Write the failing tests**

In `render.rs` tests (reuse the 5e helpers `render_to_buffer`/`row_has_*`):
```rust
#[test]
fn diagnostics_underline_the_flagged_glyphs() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
    let v = e.active().document.version;
    e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
        range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] }];
    e.active_mut().diagnostics.computed_version = v;
    crate::derive::rebuild(&mut e);
    let buf = render_to_buffer(&mut e, 40, 6);
    assert!(row_has_underline(&buf, 0), "the misspelled 'teh' is underlined");
}

#[test]
fn stale_diagnostics_are_not_painted() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
    e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
        range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] }];
    e.active_mut().diagnostics.computed_version = 999; // != current version
    crate::derive::rebuild(&mut e);
    let buf = render_to_buffer(&mut e, 40, 6);
    assert!(!row_has_underline(&buf, 0), "version-mismatched diagnostics are hidden");
}
```
Add a `row_has_underline(buf, row)` helper (scan cells for `Modifier::UNDERLINED`).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel diagnostics_underline stale_diagnostics`
Expected: FAIL (no diagnostic painting).

- [ ] **Step 3: Implement**

Generalize the row-builder fork: today it takes the `placed` path when `!hl_window.is_empty()` (search active). Change the gate to **search active OR valid diagnostics present**:
```rust
let diag_active = editor.active().diagnostics.valid_for(editor.active().document.version);
let use_placed = !hl_window.is_empty() || diag_active;
```
Before the row loop, gather the version-valid diagnostics (empty Vec if `!diag_active`):
```rust
let diag_all: &[wordcartel_core::diagnostics::Diagnostic] =
    if diag_active { &editor.active().diagnostics.diagnostics } else { &[] };
```
Inside the `placed` builder, for each glyph compute its global src `g = line_off + p.src.start .. line_off + p.src.end` and, in addition to the search-highlight test, test against the viewport-windowed diagnostics (window `diag_all` by `[lo,hi)` with the same `partition_point` pair used for search). If a glyph overlaps a diagnostic, add to its style:
```rust
style = style.add_modifier(Modifier::UNDERLINED);
style = match d.kind {
    DiagnosticKind::Spelling => style.underline_color(Color::Red),
    DiagnosticKind::Grammar  => style.underline_color(Color::Blue),
};
```
> Underline-color fallback: `Style::underline_color` compiles (ratatui 0.29). If a target terminal ignores SGR 58, the `UNDERLINED` cue still shows. A fg-tint fallback is OPTIONAL for v1 — do NOT gate the task on terminal-capability detection; the non-color underline satisfies the spec's non-color-cue requirement. Search-highlight precedence stands: a current-search REVERSED glyph keeps REVERSED (underline may still add).

When `!use_placed`, keep the existing `segs` path verbatim (true no-op for the no-search/no-diagnostics case).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p wordcartel diagnostics_underline stale_diagnostics`
Expected: PASS. Full `cargo test -p wordcartel --lib` green (existing render tests unchanged — the placed path now also fires for diagnostics, but with an empty diag set it is display-identical to the segs path; verify the search-only and no-overlay tests still pass).

- [ ] **Step 5: Commit** (subject: `feat(diag): render underline layer — generalize placed-path fork; two-tier underline_color; viewport-bounded`)

---

## Task 6: Quick-fix overlay + XOR wiring + Ctrl+. + accept/ignore/add-to-dict

**Files:**
- Create: `wordcartel/src/diag_overlay.rs`
- Modify: `wordcartel/src/editor.rs`, `wordcartel/src/app.rs`, `wordcartel/src/render.rs`, `wordcartel/src/registry.rs`, `wordcartel/src/input.rs`, `wordcartel/src/keymap.rs`, `wordcartel/src/mouse.rs`, `wordcartel/src/save.rs`, `wordcartel/src/diagnostics_run.rs` (dictionary append)

**Interfaces:**
- Consumes: `Diagnostic`, `DiagStore`, `commands::build_range_replace`, `editor.apply`, the 5e overlay-XOR + reduce-fall-through templates.
- Produces: `DiagOverlay { anchor: Diagnostic, selected: usize, buffer_id: BufferId }`; `Editor.diag: Option<DiagOverlay>`; `open_diag`; command `quick_fix`.

- [ ] **Step 1: Write the failing test**

In `app.rs` tests:
```rust
#[test]
fn quick_fix_applies_suggestion_as_undoable_edit() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
    let v = e.active().document.version;
    e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
        range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec!["the".into()] }];
    e.active_mut().diagnostics.computed_version = v;
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // cursor inside "teh"
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    reduce(Msg::Input(press(KeyCode::Char('.'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert!(e.diag.is_some(), "Ctrl+. opens the quick-fix overlay on the diagnostic");
    reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "the cat\n");
    assert!(e.diag.is_none(), "overlay closes after apply");
    assert!(e.active_mut().undo(), "the fix is one undo unit");
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "teh cat\n");
}

#[test]
fn open_diag_clears_siblings_and_open_others_clear_diag() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("x\n", None, (80, 24));
    let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] };
    e.open_diag(d);
    assert!(e.diag.is_some());
    e.open_palette();
    assert!(e.diag.is_none(), "open_palette clears diag");
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p wordcartel quick_fix_applies open_diag_clears` → FAIL.

- [ ] **Step 3: Implement `DiagOverlay`** (`diag_overlay.rs`)

```rust
use wordcartel_core::diagnostics::Diagnostic;
use crate::editor::BufferId;

pub struct DiagOverlay { pub anchor: Diagnostic, pub selected: usize, pub buffer_id: BufferId }
impl DiagOverlay {
    pub fn new(anchor: Diagnostic, buffer_id: BufferId) -> Self { DiagOverlay { anchor, selected: 0, buffer_id } }
    /// rows = suggestions… then "ignore once", "add to dictionary"
    pub fn row_count(&self) -> usize { self.anchor.suggestions.len() + 2 }
    pub fn up(&mut self) { if self.selected > 0 { self.selected -= 1; } }
    pub fn down(&mut self) { if self.selected + 1 < self.row_count() { self.selected += 1; } }
    pub fn is_ignore(&self) -> bool { self.selected == self.anchor.suggestions.len() }
    pub fn is_add_dict(&self) -> bool { self.selected == self.anchor.suggestions.len() + 1 }
    pub fn chosen_suggestion(&self) -> Option<&str> { self.anchor.suggestions.get(self.selected).map(|s| s.as_str()) }
}
```

- [ ] **Step 4: Editor field + `open_diag` + XOR**

In `editor.rs`: add `pub diag: Option<crate::diag_overlay::DiagOverlay>`, init `diag: None`. Add:
```rust
pub fn open_diag(&mut self, d: wordcartel_core::diagnostics::Diagnostic) {
    self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None; self.search = None;
    self.pending_keys.clear(); self.pending_mark = None;
    let bid = self.active().id;
    self.diag = Some(crate::diag_overlay::DiagOverlay::new(d, bid));
}
```
Add `self.diag = None;` to `open_minibuffer`/`open_prompt`/`open_palette`/`open_search`. Add `editor.diag = None;` to the menu registry handler (registry.rs:177), the mouse click-outside arms (mouse.rs), and `save.rs` reload paths.

- [ ] **Step 5: Register `quick_fix` + bind keys**

`registry.rs`: register `quick_fix` (opens the overlay for the diagnostic at the cursor — handler finds `editor.active().diagnostics` whose range contains the caret; if none, status "no diagnostic here"), plus `diag_next`/`diag_prev`/`recheck_diagnostics` (Task 7 uses the motions; register all here). CUA preset (keymap.rs): `("ctrl-.", "quick_fix"), ("f8", "diag_next"), ("shift-f8", "diag_prev")`. Mirror in `input.rs` `key_to_command_id`.

- [ ] **Step 6: reduce() diag overlay interception**

Add a `diag` branch in `reduce` after the `search` branch, before normal dispatch, mirroring the search branch's structure (intercept ONLY `Msg::Input(Event::Key(_))`, let non-key fall through):
```rust
if editor.diag.is_some() {
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == KeyEventKind::Press {
            match k.code {
                KeyCode::Up => { editor.diag.as_mut().unwrap().up(); }
                KeyCode::Down => { editor.diag.as_mut().unwrap().down(); }
                KeyCode::Esc => { editor.diag = None; }
                KeyCode::Enter => { diag_apply_selected(editor, clock); }
                _ => {}
            }
        }
        return !editor.quit;
    }
    // non-key messages fall through
}
```
Add `diag_apply_selected`: read the overlay; if `chosen_suggestion()` → `build_range_replace(anchor.range, suggestion, doc_len)` + `editor.apply(...)` (one undo unit), close; if `is_ignore()` → add the surface word to `editor.session_ignores`, close, re-arm a recheck; if `is_add_dict()` → append the word to the dictionary file (diagnostics_run helper) + `editor.dictionary`, close, re-arm.

- [ ] **Step 7: Render the overlay** — paint the `DiagOverlay` (message header + suggestions + ignore/add-dict rows, highlighted `selected`) using the palette/search overlay rectangle helpers; place it near the diagnostic. Add `editor.diag.is_some()` to `has_overlay`.

- [ ] **Step 8: Run to verify they pass** — `cargo test -p wordcartel quick_fix_applies open_diag_clears` → PASS; full `--lib` green.

- [ ] **Step 9: Commit** (subject: `feat(diag): quick-fix overlay (Ctrl+.) — accept/ignore/add-to-dict + XOR wiring + undoable apply`)

---

## Task 7: Next/prev-diagnostic motions + recheck command

**Files:**
- Modify: `wordcartel/src/app.rs` (motions), `wordcartel/src/registry.rs` (recheck handler)

**Interfaces:**
- Consumes: `Buffer.diagnostics.diagnostics`, `nav::ensure_visible`, `derive::rebuild`, `Selection::single`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn diag_next_prev_move_caret_with_wrap() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("teh cat adn dog\n", None, (80, 24));
    let v = e.active().document.version;
    e.active_mut().diagnostics.diagnostics = vec![
        wordcartel_core::diagnostics::Diagnostic { range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
        wordcartel_core::diagnostics::Diagnostic { range: 8..11, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
    ];
    e.active_mut().diagnostics.computed_version = v;
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let f8 = Event::Key(KeyEvent { code: KeyCode::F(8), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    reduce(Msg::Input(f8.clone()), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert_eq!(e.active().document.selection.primary().to(), 8, "F8 moves to the next diagnostic");
    reduce(Msg::Input(f8), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert_eq!(e.active().document.selection.primary().to(), 0, "F8 wraps to the first");
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL (motions not wired).

- [ ] **Step 3: Implement** — the `diag_next`/`diag_prev` command handlers (registered in Task 6) move the caret: find the first diagnostic with `range.start > caret` (next) or last with `range.start < caret` (prev), wrap if none; set `selection = Selection::single(d.range.start)`, `derive::rebuild` + `nav::ensure_visible`. `recheck_diagnostics` arms `recheck_due_at = now` (immediate) so the next loop tick dispatches. (These run via the normal command dispatch — no reduce special-casing needed since no overlay is open.)

- [ ] **Step 4: Run to verify it passes** — PASS; full `cargo test -p wordcartel --lib` + `cargo test -p wordcartel-core` green; zero warnings.

- [ ] **Step 5: Commit** (subject: `feat(diag): next/prev-diagnostic motions (F8/Shift+F8) + recheck command`)

---

## Self-Review (completed by plan author)

**Spec coverage:** §3.1 core `check` → T1. §3.2 DiagStore/overlay + §3.3 XOR → T3/T6. §4 data flow → T4. §4.1 debounce (loop deadline + pure helper) → T3/T4. §4.2 worker (msg_tx, version-gate, in_flight) → T4. §4.3 accept = ChangeSet edit → T6. §5.1 render (generalized placed-path, two tiers, viewport-bounded, no-op) → T5. §5.2 quick-fix overlay → T6. §5.3 keys → T6/T7. §6 config → T2. §7 perf (off-hot-path, debounce, version-gate, viewport-bounded) → T4/T5. §8 error handling (Harper panic → empty via catch_unwind; stale discard; no diagnostic at cursor; out-of-bounds clamp on apply) → T1/T4/T6. §9 testing → every task. §9.3 build gate → T1.

**Known deviation flagged for review:** T5 makes the fg-tint underline fallback OPTIONAL (the non-color `UNDERLINED` cue satisfies the spec's non-color-cue requirement); if the reviewer wants the fg-tint fallback in v1, it's a small addition.

**Type consistency:** `Diagnostic{range,kind,message,suggestions}`, `DiagnosticKind{Spelling,Grammar}`, `CheckOpts{grammar,ignore_words}`, `DiagStore{diagnostics,computed_version,recheck_due_at,in_flight_version}`, `next_deadline`/`diag_due`/`valid_for`/`arm`, `Msg::DiagnosticsDone{buffer_id,version,diagnostics}`, `dispatch_diagnostics`/`apply_diagnostics_done`, `DiagOverlay{anchor,selected,buffer_id}` are used identically across tasks.

**Placeholder scan:** the `check` adapter bodies (`harper_lints`/`classify`/`char_span_to_bytes`) and the curated grammar allow-list are explicitly Harper-version-resolved-at-the-gate, with the signature + tests fixed — bounded, not open TODOs (same discipline 5e used for the regex-cursor call site).
