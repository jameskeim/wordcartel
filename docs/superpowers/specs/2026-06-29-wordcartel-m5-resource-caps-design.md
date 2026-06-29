# M5 — Resource Caps — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-29
**Parent:** Hardening campaign workstream **M5**
(`docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`).
**Crates:** `wordcartel` shell (most caps) + `wordcartel-core` (undo-history cap only).

## Goal

Bound every place the program's memory or work can grow without limit, so a pathological
or accidental input cannot OOM, hang, or freeze the UI. This is both a robustness pillar
and the **gate for M7's fuzz CI** — without resource caps, fuzzing is an OOM/timeout
exercise. The caps are fixed safety rails (no config), centralized for auditability, and
chosen so they never bite realistic prose/markdown editing.

## Background (current state)

Already bounded:
- **Filter** subprocess output — `max_output`, default `1 << 20` (**1 MiB**) per invocation
  (`filter.rs`). *Too low* for the document sizes M5 will allow (see Decision 2).
- **Paste** — `PASTE_MAX_BYTES = 8 MiB`, `OSC52_MAX_ENCODED = 100_000` (`clipboard.rs`).
- **Export** — already uses a 64 MiB output cap (`export.rs:120`).

Unbounded today (the M5 targets):
- **Document-open size** — `fs::read(path)` slurps the whole file (`file.rs:60`).
- **Undo history** — `revisions: Vec<Revision>` grows forever (`history.rs:52`, **core**).
- **Search matches** — `matches: Vec<Match>` unbounded (`search_overlay.rs`).
- **Transform output** — in-process `run_transform` result has no cap (`transform.rs:185`).
- **Scratch/session size** — session caps *entry count* (`max_entries: 200`) but not the
  *scratch buffer content* it persists (`ScratchState`, `state.rs`).

There is **no central limits module** — caps are scattered (filter field, clipboard consts).

## Decisions (from brainstorm)

1. **Central `limits` module** (`wordcartel/src/limits.rs`) holds every shell-side quota as
   a named `const`; existing scattered caps (filter, paste) re-point here for one auditable
   place. Core's undo cap is the one exception — it lives in `wordcartel-core` (core cannot
   depend on a shell module). **All fixed constants, no config** (defer config to a real
   need; M5 is pure safety rails).
2. **One 64 MiB master document bound** unifies the document-loading / document-producing
   operations: `MAX_OPEN_BYTES = MAX_FILTER_OUTPUT = MAX_TRANSFORM_OUTPUT = 64 MiB`. Story:
   *"no single open/filter/transform operation loads or produces more than 64 MiB."* This
   also **fixes a latent bug**: filter's current 1 MiB output cap would spuriously refuse
   filtering a large (up-to-64-MiB) document through e.g. `sort`/`fmt`; raising it to 64 MiB
   (matching export) makes large-document filters work while staying bounded. 64 MiB is
   ~12× the largest novel ever written, so no real prose document approaches it, and the
   ceiling also bounds the worst-case layout pass (responsiveness).
3. **Refuse on the input/output edges, degrade on the internal caches.**
   - *Refuse* (a partial result would be wrong/corrupt): document-open, transform output.
   - *Degrade* (dropping excess is harmless): undo history, search matches, session size.
4. **Undo history is bounded by BYTES, not count**, with a most-recent floor: drop oldest
   revisions until under `MAX_UNDO_BYTES`, but always keep ≥1 (so even one giant edit is
   undoable once). Bytes — not count — is what actually bounds memory.
5. **Undo eviction is "louder"**: a one-time status hint when oldest is dropped. Core
   exposes `History.last_evicted` (count dropped on the last commit); the shell reads it
   after the edit it submitted and shows the hint. Core stays the source of truth; `commit`'s
   return type is unchanged (minimal ripple).

## The caps (summary)

| Cap | Const | Value | Behavior | Enforced at |
|---|---|---|---|---|
| Document-open size | `MAX_OPEN_BYTES` | 64 MiB | **Refuse** → `OpenError::TooLarge` | `file::open` — metadata len check BEFORE read |
| Filter output | `MAX_FILTER_OUTPUT` | 64 MiB | Refuse (existing `FilterError`) | filter call-site defaults (was 1 MiB) |
| Transform output | `MAX_TRANSFORM_OUTPUT` | 64 MiB | **Refuse** → `TransformError` | post-`run_transform` size check |
| Undo history | `MAX_UNDO_BYTES` (core) | 64 MiB | **Degrade** → evict oldest, keep ≥1, loud hint | `History::commit`/`commit_coalescing` |
| Search matches | `MAX_SEARCH_MATCHES` | 100_000 | **Degrade** → stop at N, "first N" | `search_overlay` match collection |
| Scratch/session size | `MAX_SESSION_BYTES` | 8 MiB | **Degrade** → drop scratch, keep metadata | `state::save_in` |
| Paste (re-homed) | `PASTE_MAX_BYTES` / `OSC52_MAX_ENCODED` | 8 MiB / 100_000 | unchanged | `clipboard.rs` |

## Components

### 1. `wordcartel/src/limits.rs` (new) + core undo const

```rust
//! Central resource quotas (M5). The one auditable place for "the program is bounded
//! here". All fixed safety rails — refuse on input/output edges, degrade on caches.

/// Refuse opening a file larger than this (checked via metadata, before any read).
pub const MAX_OPEN_BYTES: u64 = 64 * 1024 * 1024;
/// Ceiling on a single filter subprocess's output (was 1 MiB; raised so a whole-document
/// filter on a large doc is not spuriously refused).
pub const MAX_FILTER_OUTPUT: usize = 64 * 1024 * 1024;
/// Ceiling on a single in-process transform's output.
pub const MAX_TRANSFORM_OUTPUT: usize = 64 * 1024 * 1024;
/// Stop collecting search matches past this (navigation needs far fewer; bounds the
/// "everything matches `e`" scan + the match vector).
pub const MAX_SEARCH_MATCHES: usize = 100_000;
/// Skip persisting a serialized session larger than this (drop scratch content first).
pub const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;

/// Paste consts re-homed here for auditability (values unchanged from clipboard.rs).
pub const PASTE_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const OSC52_MAX_ENCODED: usize = 100_000;
```

Core (in `wordcartel-core`, near `history.rs`):
```rust
/// Undo-history memory budget: evict oldest revisions past this, always keeping ≥1.
pub const MAX_UNDO_BYTES: usize = 64 * 1024 * 1024;
```

### 2. Document-open size cap (`file.rs::open`)

Before `fs::read`, stat the file and refuse if too large — so a 10 GB file is never slurped
into memory:
```rust
pub fn open(path: &Path) -> Result<String, OpenError> {
    let label = path.display().to_string();
    // Size guard BEFORE reading — never slurp an oversized file.
    if let Ok(meta) = fs::metadata(path) {
        if meta.is_file() && meta.len() > crate::limits::MAX_OPEN_BYTES {
            return Err(OpenError::TooLarge { label, size: meta.len(), limit: crate::limits::MAX_OPEN_BYTES });
        }
    }
    // ... existing fs::read + binary/dir/utf-8 handling unchanged ...
}
```
New `OpenError::TooLarge { label, size, limit }` variant, with a `#[error]` Display like the
existing binary/symlink refusals (e.g. `"{label}: too large ({size} bytes > {limit} limit)"`).
The metadata read is best-effort: if `metadata` fails, fall through to the existing `fs::read`
path (which maps its own errors) — we do not turn a stat failure into a refusal.

### 3. Undo byte budget (core `history.rs`)

- `ChangeSet::stored_bytes(&self) -> usize` — sum of the byte lengths of the `Insert`
  payloads (the only heap-held text; `Retain`/`Delete` are counts). This is the memory a
  changeset actually holds.
- `History` gains `bytes: usize` (running total of all retained revisions' stored bytes)
  and `last_evicted: usize` (revisions dropped on the most recent commit; reset to 0 each
  commit when nothing is dropped).
- A `Revision`'s cost = Σ over its `edits` of `changes.stored_bytes() + inverse.stored_bytes()`.
- `commit` / `commit_coalescing`: after recording the new (or coalesced) revision, update
  `bytes`; while `bytes > MAX_UNDO_BYTES` **and** `revisions.len() > 1`, pop the oldest
  revision, subtract its cost, and increment `last_evicted`. **Coalescing caveat:** when
  `commit_coalescing` merges into the existing last revision, recompute that revision's cost
  (subtract its old cost, add the new) so `bytes` stays accurate.
- The shell, immediately after submitting the edit, reads
  `editor.active().document.history.last_evicted` and — when `> 0` — sets the one-time status
  hint `"Undo history full — oldest dropped"`. (The shell's edit path is
  `editor.rs:183`'s `document.history.commit_coalescing(...)`; the field is a direct read
  right after, not polling.)

### 4. Search match cap (`search_overlay.rs`)

The match collector stops once it has gathered `MAX_SEARCH_MATCHES`, and records that it
was capped:
- Add a `capped: bool` to the search cache (set when collection hit the ceiling).
- Navigation (next/prev/wrap) operates over the capped set unchanged.
- The search status/echo indicates `"first 100000 matches"` (or similar) when `capped`.

### 5. Session size cap (`state.rs::save_in`)

```rust
pub fn save_in(&self, dir: &Path) -> std::io::Result<()> {
    let mut text = toml::to_string(self).map_err(...)?;
    if text.len() > crate::limits::MAX_SESSION_BYTES {
        // The scratch buffer content is the only part that can be large. Drop it and
        // re-serialize the (tiny) per-path metadata, so cursor positions still persist.
        let trimmed = SessionState { scratch: None, ..self.clone() };
        text = toml::to_string(&trimmed).map_err(...)?;
        if text.len() > crate::limits::MAX_SESSION_BYTES {
            return Ok(()); // metadata alone still over cap (shouldn't happen) → skip persist
        }
    }
    // ... existing atomic write of `text` (file::save_atomic_bytes) unchanged ...
}
```
The live scratch buffer is untouched — only its cross-session *persistence* is skipped when
oversized. Session load already degrades gracefully (a missing/absent scratch → empty).

### 6. Transform output cap (`transform.rs`)

`run_transform` (or the `merge_transform_into` consumer) checks the produced output size and
refuses over-cap, parallel to filter:
```rust
let out = run_transform(kind, input, width)?;
if out.len() > crate::limits::MAX_TRANSFORM_OUTPUT {
    return Err(TransformError::OutputTooLarge { limit: crate::limits::MAX_TRANSFORM_OUTPUT });
}
```
New `TransformError::OutputTooLarge` (or reuse an existing output-error shape if present),
rendered to the status line. Refuse — a truncated transform would be silent corruption.

### 7. Filter output cap re-home (`filter.rs` + call sites)

Replace the production `max_output: 1 << 20` defaults (the real call sites — `app.rs:674`
and the filter builders) with `crate::limits::MAX_FILTER_OUTPUT` (64 MiB). **Leave the
test-only small caps** (e.g. `max_output: 64`) that exist to exercise the cap behavior.
This is a deliberate behavior change (a filter may now emit up to 64 MiB before being
killed, vs 1 MiB) that fixes the spurious-refusal-on-large-docs bug.

### 8. Paste const re-home (`clipboard.rs`)

Re-point `clipboard.rs`'s `PASTE_MAX_BYTES` / `OSC52_MAX_ENCODED` to `limits.rs` (values
unchanged) so every quota is in one place. Pure auditability; no behavior change.

## Error handling

- `OpenError::TooLarge` and `TransformError::OutputTooLarge` → status line, like the
  existing binary/symlink/filter refusals. No partial document/result is produced.
- Degrade paths never error: undo evicts (with the one-time `last_evicted` hint), search
  caps collection (with a "first N" indication), session skips/trims persistence silently.
- The open size-guard's metadata read is best-effort — a stat failure falls through to the
  existing read path, never a spurious refusal.

## Data flow (unchanged in the common case)

Normal editing is far below every cap, so behavior is identical to today. The caps only fire
on pathological inputs (a giant file, a runaway filter, a search matching everything, a
multi-megabyte scratch, an undo history that has accumulated tens of MiB). Each fires at its
single enforcement point above with the specified refuse/degrade behavior.

## Testing strategy

- **Open:** a file at `MAX_OPEN_BYTES + 1` → `OpenError::TooLarge`, and the bytes are NOT
  read (assert via a path whose content would otherwise fail differently, or a size-only
  check); a file at the limit opens normally. (Use a sparse/truncate-created large file so
  the test is fast.)
- **Undo:** `ChangeSet::stored_bytes` returns the inserted-byte count; committing revisions
  past `MAX_UNDO_BYTES` evicts oldest, keeps ≥1, and `last_evicted` reports the count; a
  single over-budget revision is retained (keep-latest); coalescing keeps `bytes` accurate.
- **Search:** a buffer with > `MAX_SEARCH_MATCHES` hits caps the vector at the ceiling and
  sets `capped`; navigation still works.
- **Session:** an over-cap scratch is dropped but per-path metadata still persists; a
  normal session round-trips unchanged.
- **Transform:** an over-cap transform output → `TransformError::OutputTooLarge`, document
  unchanged.
- **Filter:** a filter producing > 1 MiB but < 64 MiB now SUCCEEDS (pins the raised cap);
  the existing small-cap tests still refuse.
- These caps are also what makes M7's fuzz targets bounded — M5 gates that CI.

## Out of scope (deferred)

- **Config exposure** of any cap (fixed consts; add config only when a real need appears).
- **Per-operation time limits** beyond what exists (filter already has `limit_time`); not
  extending time-budgeting in M5.
- **Plugin output caps** — plugins do not exist yet (Effort P). The central `limits` module
  is where their quota will live; M5 only establishes the module + the current surfaces.
- The **fuzz harness itself** (M7) — M5 only provides the caps that make it safe.
- Tightening the existing **paste** cap or **export** cap (left at their current values).

## New code surface (checklist for the plan)

- `wordcartel/src/limits.rs` (new): the shell consts (open/filter/transform/search/session
  + re-homed paste).
- `wordcartel/src/lib.rs`: `pub mod limits;`.
- `wordcartel-core` (`history.rs` or a core consts location): `MAX_UNDO_BYTES`;
  `ChangeSet::stored_bytes`; `History.{bytes, last_evicted}`; eviction in
  `commit`/`commit_coalescing` (incl. the coalescing recompute).
- `wordcartel/src/file.rs`: `OpenError::TooLarge` + the pre-read size guard in `open`.
- `wordcartel/src/transform.rs`: `TransformError::OutputTooLarge` + the post-run size check.
- `wordcartel/src/search_overlay.rs`: collection cap + `capped` flag + status indication.
- `wordcartel/src/state.rs`: the over-cap drop-scratch-then-skip logic in `save_in`.
- `wordcartel/src/filter.rs` + `app.rs`: re-point production `max_output` defaults to
  `MAX_FILTER_OUTPUT` (leave test-only small caps).
- `wordcartel/src/clipboard.rs`: re-point paste consts to `limits.rs`.
- The shell-side one-time "undo history full" hint reading `History.last_evicted`.
- Tests per the testing strategy.
