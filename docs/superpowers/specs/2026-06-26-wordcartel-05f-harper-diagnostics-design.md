# Wordcartel Effort 5f — Harper Diagnostics — Design

**Date:** 2026-06-26
**Status:** Design (pre-plan)
**Effort:** 5f (after 5e search & replace; before 5g outline/folding)

## 1. Summary

Background grammar + spell **diagnostics** powered by **Harper** (`harper-core`,
a Rust-native, in-process checker). Diagnostics are **markers, never buffer
edits** (roadmap §3.5): misspellings and a curated set of grammar/style issues
are **underlined** in two visual tiers, re-checked **live on a debounce** off the
input loop, and acted on through a **per-diagnostic quick-fix overlay** (view the
message + suggestions, accept a fix as an undoable edit, ignore, or add to a
personal dictionary). Next/prev-diagnostic motions walk the document.

Harper runs in a new IO-free `wordcartel-core::diagnostics` module (pure text
analysis → `Vec<Diagnostic>`, unit-tested deterministically). The shell debounces
edits, runs the check on the background worker (the 4b `Executor`), surfaces
results via a **version-gated** `Msg::DiagnosticsDone`, stores them per-buffer,
projects the markers through `ColMap` in render (the same byte-range projection
5e built for search highlights), and owns the overlay + dictionary file I/O.
**Typing is never blocked.**

## 2. Goals / Non-Goals

### Goals
- In-process **Harper** check (`harper-core`) → `Diagnostic{range,kind,message,suggestions}`.
- **Two tiers:** `Spelling` and `Grammar` (a curated, configurable subset of Harper's linters).
- **Live-debounced** re-check (~400 ms idle) on the background worker; never per-keystroke; never blocks input.
- **Version-gated** results (stale checks discarded); markers **hidden while the buffer is dirtier than the last check** (no stale-offset remapping).
- **Underline** markers projected through `ColMap.placed`, two visual tiers, viewport-bounded.
- **Quick-fix overlay** (`Ctrl+.`): message + suggestions; accept = `ChangeSet` edit (undoable); `[ignore]`; `[add to dict]`.
- **Next/prev-diagnostic motions** (`F8` / `Shift+F8`).
- **Personal dictionary** (persisted file Harper loads) + `[diagnostics]` config (enable, linter set, debounce ms, dict path).
- Spellcheck **enabled by default**; deterministic in tests via `InlineExecutor`.

### Non-Goals (v1)
- Multi-language / per-document language selection (Harper default locale only).
- A persistent "problems panel" listing all diagnostics (Effort 6+ chrome).
- Incremental / region-limited re-check (full-doc per debounce; cap deferred).
- Stale-marker offset remapping through edits (we hide-then-replace instead).
- Custom user-authored rules / linter authoring (advanced).
- Auto-fix-all / bulk accept (accept is per-diagnostic only).

## 3. Architecture

Functional-core / imperative-shell.

```
wordcartel-core (IO/thread-free, #![forbid(unsafe_code)])
  diagnostics.rs  NEW — Diagnostic/DiagnosticKind/CheckOpts; check(text,&opts)
                        wraps harper-core, maps lints → Diagnostic. Pure;
                        unit-tested on fixed sentences. No IO, no threads.

wordcartel (shell)
  diagnostics_run.rs NEW — debounce arming, worker dispatch, version-gated
                           apply of Msg::DiagnosticsDone, personal-dictionary
                           file load/append (the IO Harper itself doesn't do).
  diag_overlay.rs    NEW — DiagOverlay (quick-fix picker) state machine.
  editor.rs          + per-Buffer `diagnostics: DiagStore`; `diag: Option<DiagOverlay>`.
  app.rs             + Msg::DiagnosticsDone; reduce() apply (version-gate) +
                       debounce deadline folded into the existing recv_timeout;
                       diag overlay interception (XOR); next/prev motions.
  render.rs          + diagnostic underline layer (ColMap projection) + overlay paint.
  registry.rs        + commands (recheck, quick_fix, diag_next, diag_prev, add_to_dict).
  input.rs/keymap.rs + Ctrl+. / F8 / Shift+F8 binds (CUA preset).
  config.rs          + [diagnostics] section.
```

### 3.1 Core: `wordcartel-core::diagnostics`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagnosticKind { Spelling, Grammar }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub range: std::ops::Range<usize>,   // byte range in the source text
    pub kind: DiagnosticKind,
    pub message: String,                 // human-readable (Harper's lint message)
    pub suggestions: Vec<String>,        // replacement texts, best-first (may be empty)
}

pub struct CheckOpts<'a> {
    pub grammar: bool,                   // false = spelling-only (DiagnosticKind::Grammar suppressed)
    pub ignore_words: &'a std::collections::HashSet<String>, // personal dictionary + session ignores
    // (the enabled-linter set is applied here; v1 maps Harper lint categories → Spelling|Grammar)
}

/// Run Harper over `text`, return diagnostics sorted ascending by range.start,
/// non-overlapping where Harper guarantees it (overlaps tolerated by render).
/// Pure: no IO, no threads, no global state. Deterministic for a given
/// (text, opts). Words in `ignore_words` (case-insensitive) are dropped.
pub fn check(text: &str, opts: &CheckOpts) -> Vec<Diagnostic>;
```

Mapping: Harper exposes lints with character spans, a kind/category, a message,
and suggestions. `check` converts Harper char-spans to **byte** ranges (UTF-8
aware), classifies each lint as `Spelling` (Harper's spell lint) or `Grammar`
(everything else in the enabled set), drops ignored words, and sorts. The
**enabled curated grammar set** is a small allow-list resolved here (so the
shell config names linters; core applies them).

> **Harper API note:** the exact `harper-core` entry points
> (`Document`/`Linter`/`lint`/span types, dictionary injection) are version-
> sensitive — settled at the **Task-1 build gate** (§9.3), same discipline as
> `regex-cursor` in 5e. The `Diagnostic` contract and the `check` signature are
> fixed; only the body adapts to the resolved crate.

### 3.2 Shell: store, overlay, run

```rust
// per-Buffer (editor.rs)
pub struct DiagStore {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,   // buffer version the diagnostics were computed against
    pub recheck_due_at: Option<u64>, // ms timestamp; armed on edit, consumed by the loop
    pub in_flight_version: Option<u64>, // a check dispatched for this version, awaiting result
}

// global overlay (editor.rs), XOR with prompt/palette/menu/minibuffer/search
pub struct DiagOverlay {
    pub anchor: Diagnostic,      // the diagnostic being fixed (snapshot)
    pub selected: usize,         // index into [suggestions…, ignore, add-to-dict]
    pub buffer_id: BufferId,
}
```

- **Markers are valid only when `computed_version == buffer.version`.** Render
  paints diagnostics only then; after any edit (version bump) the markers are
  **hidden** until the next debounced re-check lands a fresh set. No remapping.
- The **session ignore set** + the **personal dictionary** (loaded from the
  config path at startup) compose into `CheckOpts.ignore_words`.

## 4. Data flow

```
edit → buffer.version bumps → diag store: recheck_due_at = now + debounce_ms,
                              markers hidden (version mismatch)
loop  → recv_timeout(min(existing deadlines, recheck_due_at)) wakes
      → if now >= recheck_due_at and no in_flight for this version:
            dispatch a diagnostics Job on the Executor for (buffer_id, version, text snapshot)
            in_flight_version = version
worker→ core::diagnostics::check(text, opts) → Msg::DiagnosticsDone{buffer_id, version, diagnostics}
reduce→ if version == current buffer.version: store diagnostics, computed_version = version,
          clear in_flight → repaint underlines.  Else: discard (stale).
Ctrl+. on a marker → open DiagOverlay(diagnostic under cursor)
      → Enter on a suggestion → apply ChangeSet replace(range → suggestion) [undoable] → close
      → [ignore] → add word to session ignore set → re-check
      → [add to dict] → append word to dictionary file + ignore set → re-check
F8 / Shift+F8 → move cursor to next/prev diagnostic.range (wrap), pin via ensure_visible
```

### 4.1 Debounce (reuses the existing loop timeout)
The main loop already computes a deadline and calls `msg_rx.recv_timeout(deadline)`
(app.rs ~1228-1230, today driven by `scrollbar_until_ms`). Fold
`recheck_due_at` into that `min()`: the loop wakes when the debounce elapses,
the wake handler checks "due and not in-flight," and dispatches. **No new timer,
thread, or tick source.**

### 4.2 Worker dispatch
The diagnostics check runs on the **existing `Executor`** (single background
worker, FIFO) as a new `Job`. The result is surfaced as `Msg::DiagnosticsDone`
and **version-gated** in `reduce` exactly like `FilterDone`/`TransformDone`.
Only the latest version's result is kept; superseded results are discarded. (The
worker is also careful not to be starved by overlays — the 5e
"don't-swallow-background-messages" fix applies: the diag overlay's reduce branch
must let non-key messages fall through, like minibuffer/search.)

### 4.3 Accept = ordinary edit
Accepting a suggestion replaces `diagnostic.range` with the suggestion text via
the standard `commands::build_range_replace` → `editor.apply(txn, edit, EditKind::Other, clock)`
path — **one undo unit**, marks/selection remapped by `Buffer::apply` as usual.
The edit bumps the version → markers hide → next debounce re-checks.

## 5. UI / keys / rendering

### 5.1 Two visual tiers
- **Spelling:** `UNDERLINED` + a spelling tier color.
- **Grammar:** `UNDERLINED` + a distinct grammar tier color.
- Tier color is applied as the **underline color** (crossterm `SetUnderlineColor`,
  SGR 58) **where the terminal supports it**; otherwise it falls back to a
  **foreground tint** on the underlined glyphs. The non-color cue (the underline
  itself) always survives (roadmap §4 mandates a non-color cue). Capability
  detection + fallback is a **plan verification item**.
- Markers project through `ColMap.placed[].src` (global = `line_start(buf,l)+placed.src`),
  **viewport-bounded** (window the sorted diagnostics to the visible byte span,
  the same partition_point technique as 5e's highlight). Empty store / version
  mismatch = **true no-op** (existing render tests unchanged).
- Layering: diagnostic underline composes with markdown style and search
  highlight; precedence — a current **search** highlight (REVERSED) wins visually,
  diagnostic underline otherwise adds to the glyph's style.

### 5.2 Quick-fix overlay
```
  the cat sat on teh mat              ← cursor on "teh"
  ┌─ Spelling · "teh" ─────────────┐
  │ > the                          │
  │   tea                          │
  │   ten                          │
  │ ─────────────                  │
  │   ignore once                  │
  │   add to dictionary            │
  └────────────────────────────────┘
   ↑/↓ select · Enter apply · Esc
```
- Opened by `Ctrl+.` when the cursor is on (or adjacent to) a diagnostic range;
  if none, status "no diagnostic here".
- XOR overlay (clears/cleared-by the other overlays; bound to `buffer_id`).
- Enter on a suggestion → apply edit; on `ignore once` → session-ignore that
  surface form; on `add to dictionary` → append + persist. Esc cancels.

### 5.3 Keys (CUA preset — production path is keymap.rs, mirror the test table)
| Key | Action |
|-----|--------|
| `Ctrl+.` | open quick-fix overlay for the diagnostic at the cursor |
| `F8` | move to next diagnostic (wrap) |
| `Shift+F8` | move to previous diagnostic (wrap) |
| (palette) `Recheck diagnostics` | force an immediate re-check |
| (overlay) `↑/↓` `Enter` `Esc` | select / apply / cancel |

> Collision check (plan): `Ctrl+.`, `F8`, `Shift+F8` must be free in the CUA
> preset (cross-check keymap.rs; the search effort took `Ctrl+F/R`, `F3`,
> `Shift+F3`). Terminal delivery of `Ctrl+.` and `Shift+F8` is a portability
> test item; provide palette-reachable fallbacks.

## 6. Config

```toml
[diagnostics]
enabled = true            # spellcheck on by default
grammar = true            # false → spelling-only (tier 2 suppressed)
debounce_ms = 400         # idle delay before re-check; min-clamped (e.g. >= 100)
dictionary = "~/.config/wordcartel/dictionary.txt"  # one word per line; created on first add
# linters = ["spelling", "repeated_words", "sentence_capitalization", ...]  # curated allow-list; omitted = default set
```
Validation (like 5d/5a `load()` → `warns`): clamp `debounce_ms`; unknown linter
names warn + ignore; missing dictionary file is not an error (created on first
add-to-dict). `enabled=false` → no dispatch, no markers, true no-op.

## 7. Performance / responsiveness (the #1 priority)

- **Check is off the hot path:** always on the worker; typing never waits.
- **Debounce** collapses bursts into one check per idle pause; **version-gating**
  discards superseded results; **in_flight** guard prevents piling up checks.
- **Full-doc check per pause** is acceptable to ~1–2 MB (same budget as search
  `count` and the block-tree reparse). Incremental/region-limited checking is a
  noted, deferred optimization; if profiling shows multi-MB stalls, debounce
  longer or check the changed paragraph's region.
- **Render markers viewport-bounded** (partition_point window) → O(visible).
- **No new thread or tick source** — debounce rides the existing `recv_timeout`.

## 8. Error handling

| Situation | Behavior |
|-----------|----------|
| `harper-core` panics / errors on input | check returns `vec![]` (no diagnostics) — never crash the loop; log once |
| Result arrives for a stale version | discarded (version gate); markers stay hidden until current-version result lands |
| `Ctrl+.` with no diagnostic at cursor | status "no diagnostic here"; no overlay |
| Suggestion list empty (e.g. some grammar lints) | overlay shows the message + `[ignore]`/`[add to dict]` only (no apply targets) |
| Dictionary file unreadable/unwritable | warn in status; ignore stays session-only; never crash |
| Diagnostic range out of bounds after a race | clamp to buffer length before slicing; never panic |
| `enabled=false` | no dispatch, no store, no markers, no overlay |
| Multibyte text | char↔byte conversion in `check`; render projects via ColMap (UTF-8 safe) |

## 9. Testing

### 9.1 Core (`wordcartel-core::diagnostics`) — deterministic, no threads
- Fixed sentences → expected diagnostics: a misspelling yields a `Spelling`
  diagnostic with non-empty suggestions and the correct **byte** range; a
  repeated word (`the the`) yields a `Grammar` diagnostic; `grammar=false`
  suppresses the grammar one; an `ignore_words` entry drops its diagnostic.
- Multibyte: a misspelling after `café` has the correct byte offset.
- Determinism: `check` twice on the same input → identical output.
- Sort/format: output sorted ascending by `range.start`.

### 9.2 Shell
- **Version gate:** a `DiagnosticsDone{version=N}` applied when buffer is at N
  stores; at N+1 (edited meanwhile) is discarded; markers hidden while
  `computed_version != version`.
- **Debounce:** an edit arms `recheck_due_at`; the loop deadline includes it; a
  check is dispatched once after the delay, not per keystroke; `in_flight`
  prevents a second dispatch for the same version. (Driven via `InlineExecutor` +
  a controllable clock for determinism.)
- **Accept:** applying a suggestion is one undo unit (single `undo()` reverts);
  remaps selection; bumps version.
- **Ignore / add-to-dict:** the word stops producing a diagnostic after the next
  check; add-to-dict appends to the file.
- **Render:** diagnostics underline the right glyphs via ColMap projection,
  including a diagnostic spanning concealed markdown; two tiers distinguishable;
  viewport-bounded; empty/stale store = no-op (existing render tests unchanged).
- **Overlay XOR:** opening the diag overlay clears the others and vice versa;
  buffer swap closes it; the overlay reduce branch does **not** starve
  `FilterDone`/`ExportDone`/`TransformDone`/`DiagnosticsDone` (5e lesson).
- **Motions:** `F8`/`Shift+F8` move to next/prev diagnostic with wrap.
- **Keys:** `Ctrl+.`/`F8`/`Shift+F8` bound in CUA, no collision.

### 9.3 Dependency build gate (Task 1)
Add `harper-core` to `wordcartel-core` and prove, before any feature code:
(a) it compiles against the workspace (and the pinned `ropey`/other deps don't
conflict); (b) it does not break core's `#![forbid(unsafe_code)]` (the dep may
use unsafe internally; our code may not); (c) license is acceptable
(Apache-2.0/MIT — confirm); (d) **binary-size / bundled-dictionary** impact is
acceptable (Harper ships a dictionary — measure the size cost and that it links
without a data-file path dependency); (e) settle the real lint/span/dictionary
API and that char→byte span conversion is available. If any fails, STOP and
report — contingency is a human decision (e.g. fall back to a lighter speller).

## 10. Dependencies
- **New in `wordcartel-core`:** `harper-core` (confirm exact crate name +
  version + license + dictionary-bundling at the Task-1 gate).
- No new shell dependencies (worker/overlay/render reuse existing infra).
- `ropey` pin untouched.

## 11. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| `harper-core` build/size/dictionary surprises | Task-1 build gate before feature code; measure size; contingency = lighter speller |
| Full-doc re-check stalls on multi-MB docs | off-hot-path + debounce + version-gate + in_flight guard; region-limited check deferred but noted |
| Stale/misaligned underlines after edits | markers hidden unless `computed_version == version`; never remap — hide-then-replace |
| Harper char-spans vs our byte offsets | convert in `check` (UTF-8 aware), oracle byte-range test |
| Overlay starves background worker messages | diag reduce branch lets non-key msgs fall through (explicit 5e-pattern test) |
| Two-tier underline color unsupported by terminal | non-color cue (underline) always present; fg-tint fallback; capability test item |
| `Ctrl+.` / `Shift+F8` not delivered by some terminals | palette-reachable fallbacks; documented test item |
| Default-on diagnostics perturb existing tests | dispatch only when enabled AND an Executor is wired; `InlineExecutor` determinism; render no-op when empty |

## 12. Out of scope → future
- Multi-language / per-document locale; problems panel; incremental check;
  user-authored rules; auto-fix-all; live dictionary-management UX beyond
  append-on-add.
