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
| Document-open size | `MAX_OPEN_BYTES` | 64 MiB | **Refuse** → `OpenError::TooLarge` | `file::open` — metadata fast-check **+ bounded `Read::take`** |
| Filter output | `MAX_FILTER_OUTPUT` | 64 MiB | Refuse (existing `FilterError`) | `app.rs:668-675` production builder (was 1 MiB) |
| Transform output | `MAX_TRANSFORM_OUTPUT` | 64 MiB | **Refuse** → `TransformError` | post-`run_transform` size check |
| Undo history | `MAX_UNDO_BYTES` (core) | 64 MiB | **Degrade** → evict oldest, keep ≥1, loud hint | `History::commit`/`commit_coalescing` (+ `current` fix) |
| Search matches | `MAX_SEARCH_MATCHES` | 100_000 | **Degrade** → stop at N, "first N" | core `search::all_matches` (limit param) |
| Scratch/session size | `MAX_SESSION_BYTES` | 8 MiB | **Degrade** → drop scratch, keep metadata | scratch snapshot + `state::save_in` + bounded load |
| Swap/recovery load | (reuses `MAX_OPEN_BYTES`) | 64 MiB | **Degrade** → over-cap swap treated as absent | `swap.rs` bounded reads (load/orphan-scan) |
| Paste (re-homed) | `PASTE_MAX_BYTES` / `OSC52_MAX_ENCODED` | 8 MiB / 100_000 | unchanged | `limits.rs` canonical, `clipboard.rs` re-export |

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

A metadata pre-check is necessary but **not sufficient** — a `/proc`-style or sparse file can
report length 0 yet stream gigabytes, so trusting `metadata().len()` leaves a hole. The
guard therefore (a) fast-refuses on a trustworthy metadata length, **and** (b) bounds the
actual read so even a lying-metadata file cannot slurp unbounded:
```rust
pub fn open(path: &Path) -> Result<String, OpenError> {
    let label = path.display().to_string();
    // (a) Fast refusal when metadata is trustworthy (gives an exact size in the error).
    if let Ok(meta) = fs::metadata(path) {
        if meta.is_file() && meta.len() > crate::limits::MAX_OPEN_BYTES {
            return Err(OpenError::TooLarge { label, size: Some(meta.len()), limit: crate::limits::MAX_OPEN_BYTES });
        }
    }
    // (b) Bounded read: read at most MAX_OPEN_BYTES + 1 bytes. If we got the extra byte,
    //     the file exceeds the cap regardless of what metadata claimed.
    let mut bytes = Vec::new();
    let limit = crate::limits::MAX_OPEN_BYTES;
    match fs::File::open(path) {
        Ok(f) => {
            // Read::take(limit + 1) caps the allocation; errors map as today.
            std::io::Read::take(f, limit + 1).read_to_end(&mut bytes)
                .map_err(/* existing IO/NotFound/Permission/IsDir mapping */)?;
            if bytes.len() as u64 > limit {
                return Err(OpenError::TooLarge { label, size: None, limit });
            }
        }
        Err(e) => return Err(/* existing kind mapping (NotFound/Permission/IsDir/Io) */),
    }
    // ... existing is_dir / is_binary (NUL + utf-8) checks + String::from_utf8 on `bytes` ...
}
```
New `OpenError::TooLarge { label, size: Option<u64>, limit }` variant (`size: None` when only
the bounded-read tripwire fired), with a `#[error]` Display like the existing binary/symlink
refusals (e.g. `"{label}: too large (> {limit} bytes)"`). The existing error-kind mapping for
`fs::read` is preserved by reusing it on the `File::open`/`read_to_end` errors. (The current
`open` uses `fs::read`; this changes it to `File::open` + bounded `read_to_end`, preserving
the same downstream binary/dir/utf-8 handling on the resulting `bytes`.)

### 3. Undo byte budget (core `history.rs`)

- `ChangeSet::stored_bytes(&self) -> usize` — sum of the byte lengths of the `Insert(Tendril)`
  payloads. A *delete*'s text is held in the revision's *inverse* (as an `Insert`), so summing
  both `changes.stored_bytes()` and `inverse.stored_bytes()` captures insert AND delete text.
  (The `Vec<Op>` structural overhead of `Retain(usize)`/`Delete(usize)` is excluded as
  negligible vs. the payload text — this is a memory budget on the dominant cost.)
- `History` gains `bytes: usize` (running total of all retained revisions' stored bytes)
  and `last_evicted: usize` (revisions dropped on the most recent commit; reset to 0 each
  commit when nothing is dropped).
- A `Revision`'s cost = Σ over its `edits` of `changes.stored_bytes() + inverse.stored_bytes()`.
- **Maintaining `bytes` correctly across the redo-tail truncation.** `commit` and the
  non-merge `commit_coalescing` path discard the redo tail with
  `self.revisions.truncate(self.current)` (history.rs:67, :132). The byte total MUST subtract
  the truncated revisions' costs BEFORE pushing the new one — sum `revisions[current..]`
  costs and subtract, then truncate, then push and add the new revision's cost. Otherwise
  `bytes` drifts permanently high and evicts unnecessarily.
- **Coalescing-merge path:** when `commit_coalescing` merges into the existing last revision
  (rather than pushing), recompute that revision's cost (subtract its old cost, add the new)
  so `bytes` stays accurate.
- **Eviction — and the `History.current` invariant (the load-bearing fix).** Eviction runs
  immediately AFTER a commit, where `current == revisions.len()` (the commit truncated any
  tail, pushed, and did `current += 1`, so all revisions are "applied"). While
  `bytes > MAX_UNDO_BYTES` **and** `revisions.len() > 1`: `remove(0)` the oldest revision,
  subtract its cost, **`current -= 1`**, and increment `last_evicted`. The `current -= 1` is
  mandatory: `undo`/`redo` index `revisions[current]`/`revisions[current-1]` (history.rs:79-99),
  so dropping a front revision without shifting `current` down would leave `current` pointing
  past the shortened vector and corrupt the next undo/redo. Because eviction starts at
  `current == len` and keeps ≥1 revision, `current` stays in `[1, len]` and consistent.
- The shell, immediately after submitting the edit, reads
  `editor.active().document.history.last_evicted` and — when `> 0` — sets the one-time status
  hint `"Undo history full — oldest dropped"`. (The shell's edit path is
  `editor.rs:183`'s `document.history.commit_coalescing(...)`; the field is a direct read
  right after, not polling.)

### 4. Search match cap (core `search::all_matches` + `search_overlay.rs`)

The cap must be enforced **inside the collector** (core `all_matches`, `search.rs:52-73`,
which currently pushes every match to EOF) — a post-hoc length check would not bound the
peak `Vec<Match>` allocation. So:
- Core: `all_matches` takes a `limit: usize` and stops collecting once it has `limit`
  matches, returning whether it was capped (e.g. `-> (Vec<Match>, bool)`, or a small
  `Matches { items, capped }`). The shell passes `crate::limits::MAX_SEARCH_MATCHES`.
- Shell (`SearchState::recompute`, search_overlay.rs:92-101): store the returned `capped`
  flag in the search cache alongside `matches`.
- Navigation (next/prev/wrap) operates over the capped set unchanged.
- The search status/echo indicates `"first 100000 matches"` (or similar) when `capped`.

### 5. Session size cap (scratch snapshot, `save_in`, and load)

Three points, so a large scratch is bounded *before* it is materialized, not just before it
is written:

- **At the scratch snapshot (`app.rs:2121-2127`).** Today the scratch rope is copied into a
  `String` via `sb.document.buffer.to_string()` BEFORE persistence. Guard with the rope's
  O(1) byte length first: if `sb.document.buffer.len_bytes() > MAX_SESSION_BYTES`, persist
  the session with `scratch: None` (don't build the giant `String` at all). The live scratch
  buffer is untouched — only its cross-session persistence is skipped.
- **In `save_in` (`state.rs:81`), as a belt-and-suspenders cap on the serialized TOML.** If
  `toml::to_string(self).len() > MAX_SESSION_BYTES`, re-serialize with `scratch: None`
  (`SessionState { scratch: None, ..self.clone() }`) so the tiny per-path metadata still
  persists; if even that exceeds the cap, skip the write (`Ok(())`). The atomic write
  (`file::save_atomic_bytes`) is unchanged.
- **On load (`state.rs::load_in`, ~:98-103).** Today it does
  `std::fs::read_to_string(dir.join("session.toml"))` unbounded — a pre-existing or corrupt
  oversized `session.toml` would slurp before parsing. Read it bounded (`File::open` +
  `Read::take(MAX_SESSION_BYTES + 1).read_to_string`); if over cap, treat as absent → empty
  session (the existing graceful-degradation path for a missing/unparseable session).

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
**Known limitation:** `run_transform` is in-process and materializes the full output `String`
(`opts.format(input)`) before this check, so the cap refuses *applying* an over-cap result
but does not prevent the formatter's transient allocation. That peak is acceptable because it
is bounded by the input (a document region ≤ the open cap) plus the transform's bounded
expansion — it is not unbounded. (A truly streaming transform cap is out of scope.)

### 7. Filter output cap re-home (`app.rs` production builder)

The **single production call site** is the filter-job builder at `app.rs:668-675`, which
sets `max_output: 1 << 20`. Replace that with `crate::limits::MAX_FILTER_OUTPUT` (64 MiB).
**Leave the test caps unchanged:** the intentionally tiny `max_output: 64` cap-behavior
tests (`filter.rs:440-455`, `:460-486`) must stay 64; the ordinary `1 << 20` unit tests
(`filter.rs:390-412`, `:501-509`) may stay or move to the constant — either is fine, they
are not production. This is a deliberate behavior change (a filter may now emit up to 64 MiB
before being killed, vs 1 MiB) that fixes the spurious-refusal-on-large-docs bug. The
filter output replaces a selection via the normal (M2) edit boundary; a 64 MiB replacement
is a large-but-bounded ChangeSet and undo revision — no new pipeline issue.

### 8. Paste const re-home (`limits.rs` canonical, `clipboard.rs` re-export)

Define `PASTE_MAX_BYTES` / `OSC52_MAX_ENCODED` canonically in `limits.rs` and re-export them
from `clipboard.rs` (`pub use crate::limits::{PASTE_MAX_BYTES, OSC52_MAX_ENCODED};`). Values
unchanged. The re-export means existing `crate::clipboard::PASTE_MAX_BYTES` call sites
(`app.rs:699-703`, `:2443-2444`) keep compiling with ZERO churn while the canonical
definition lives in the audit module. Pure auditability; no behavior change.

### 9. Swap / recovery load cap (`swap.rs`)

The startup recovery path reads swap files unbounded — `find_orphan_scratch_swap` reads each
candidate with `read_to_string(entry.path())` (swap.rs:176-188) and normal recovery reads the
swap with `read_to_string(&sp)` (swap.rs:241). A pre-existing or corrupt oversized swap file
would slurp on launch. Bound both reads (`File::open` + `Read::take(MAX_OPEN_BYTES + 1)
.read_to_string`); a swap that exceeds the cap is treated as **absent/unrecoverable** — the
existing graceful path (no recovery offered, `OpenNormally`/skip), never a crash or hang. A
new `MAX_SWAP_BYTES` could be introduced, but reusing `MAX_OPEN_BYTES` (a swap holds a
document-body snapshot + a small header) keeps the bound consistent with the document
ceiling.

## Error handling

- `OpenError::TooLarge` and `TransformError::OutputTooLarge` → status line, like the
  existing binary/symlink/filter refusals. No partial document/result is produced.
- Degrade paths never error: undo evicts (with the one-time `last_evicted` hint), search
  caps collection (with a "first N" indication), session skips/trims persistence silently,
  an over-cap swap is treated as absent (no recovery offered).
- The open guard's metadata pre-check is a fast path only; correctness rests on the bounded
  `Read::take` so a stat failure or a lying-metadata file can never slurp unbounded.

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
  **Critically:** after eviction, `undo` then `redo` round-trip correctly (proves `current`
  was decremented per eviction — the index invariant); and a commit that truncates a redo
  tail subtracts the truncated bytes (no drift).
- **Search:** a buffer with > `MAX_SEARCH_MATCHES` hits caps the collected vector at the
  ceiling (assert `len == MAX_SEARCH_MATCHES`, not more — proves the cap is in the collector)
  and sets `capped`; navigation still works.
- **Session:** an over-cap scratch is dropped but per-path metadata still persists; a
  normal session round-trips unchanged; an over-cap `session.toml` on load → empty session.
- **Swap:** an over-cap swap file on recovery is treated as absent (no recovery, no slurp).
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
- **A hard ceiling on the cumulative live document.** M5 bounds every *load* path (open,
  session, swap) and every *single-operation* output (filter, transform, paste). It does NOT
  impose a hard cap on the total in-memory document, which can still grow via many accepted
  edits (repeated pastes/filters) — and operations that scale with document size (render,
  layout, diagnostics' `snapshot().to_string()` at diagnostics_run.rs:53) scale with it. A
  hard live-document ceiling would require a check on every accepted edit and belongs in the
  edit-submission boundary (M2 territory), not in M5's load/output caps.

## New code surface (checklist for the plan)

- `wordcartel/src/limits.rs` (new): the shell consts (open/filter/transform/search/session
  + canonical paste consts).
- `wordcartel/src/lib.rs`: `pub mod limits;`.
- `wordcartel-core` (`history.rs` or a core consts location): `MAX_UNDO_BYTES`;
  `ChangeSet::stored_bytes`; `History.{bytes, last_evicted}`; eviction in
  `commit`/`commit_coalescing` — incl. **`current -= 1` per eviction**, the **redo-tail
  truncation byte subtraction**, and the coalescing-merge recompute.
- `wordcartel-core/src/search.rs`: `all_matches` gains a `limit` param + capped signal.
- `wordcartel/src/file.rs`: `OpenError::TooLarge { label, size: Option<u64>, limit }` + the
  metadata fast-check **and bounded `File::open`+`Read::take`** read in `open` (replacing
  `fs::read`), preserving the downstream binary/dir/utf-8 handling.
- `wordcartel/src/transform.rs`: `TransformError::OutputTooLarge` + the post-run size check.
- `wordcartel/src/search_overlay.rs`: pass `MAX_SEARCH_MATCHES` to `all_matches`, store the
  `capped` flag, status indication.
- `wordcartel/src/state.rs`: over-cap drop-scratch in `save_in` + **bounded `load_in` read**.
- `wordcartel/src/app.rs`: the scratch-snapshot `len_bytes()` guard (~:2121); re-point the
  filter builder `max_output` (`:668-675`) to `MAX_FILTER_OUTPUT`; the shell-side one-time
  "undo history full" hint reading `History.last_evicted`.
- `wordcartel/src/swap.rs`: bounded reads in `find_orphan_scratch_swap` (~:176-188) and the
  recovery read (~:241); over-cap → treated as absent.
- `wordcartel/src/clipboard.rs`: `pub use` re-export of the paste consts from `limits.rs`.
- Tests per the testing strategy.
