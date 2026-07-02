# M5 follow-ups: undo-eviction hint + bound the last document-sized reads — design

**Status:** spec-review CLEAN (Codex READY FOR PLANNING, round 4)
**Date:** 2026-07-01
**Effort:** M5 follow-ups (pre-Effort-P; two small M5 leftovers, one bundled effort)

## Context

M5 (resource caps) left two documented loose ends. This effort closes both. They
share no code but are both small "finish M5" cleanups, bundled into one branch to
amortize the gated-pipeline overhead. **Almost entirely shell-side; the one
`wordcartel-core` change is a 2-line correctness tweak in `History::undo`/`redo`
(reset `last_evicted = 0`) — see Component (a).**

- **(a) Undo-eviction hint is incomplete.** M5 bounds undo history by BYTES
  (`MAX_UNDO_BYTES = 64 MiB`, `history.rs:9`): when the retained revisions exceed
  the budget, `History::evict_to` (`history.rs:68`) drops the oldest and records
  the count in `History.last_evicted` (`history.rs:56`). Both `History::commit`
  and `commit_coalescing` call `evict_to`, so eviction is **always enforced in
  core, on every edit path** — this effort does NOT change that. The *hint* to the
  user, however, is surfaced only on the command/keystroke path (`Editor::apply`,
  `editor.rs:646`), which reads `last_evicted` and sets
  `status = "Undo history full — oldest dropped"`. The buffer-level merges
  (filter, transform, paste, replace-all, search-step, scratch-append) go through
  `Buffer::apply` directly and set their own outcome status, so they never surface
  the hint. Purely cosmetic gap.
- **(b) Three document-sized reads remain unbounded.** M5 bounded the authoritative
  load paths at read time via `File::open` + `Read::take(cap+1)` + a
  `len > cap → refuse` check (against `MAX_OPEN_BYTES = 64 MiB`, `limits.rs:5`; see
  `file::open`, `file.rs:85`). Three auxiliary reads still use whole-file
  `std::fs::read` and were left unbounded: the recovery predicate
  (`app.rs:1936`), the external-mod `fingerprint` (`save.rs:31`), and the
  save skip-unchanged comparison (`file.rs:138`). All three are cold-path,
  comparison/optimization reads (not the authoritative load), and each already
  degrades safely when the read fails.

## Goals

- Surface the undo-eviction hint uniformly on ALL active-buffer edit paths
  (keystroke AND every buffer-level merge) via a single reduce-level seam,
  single-sourcing the hint text.
- Bound the three remaining document-sized reads at the `MAX_OPEN_BYTES` cap, each
  falling back to its existing safe degradation on over-cap (no user-facing error).
- No behavior change beyond the hint surfacing + the allocation cap; the only core
  change is the `last_evicted` reset in `undo`/`redo` (Component (a)).

## Non-goals

- No change to the eviction MECHANISM (core `evict_to` and the byte budget are
  unchanged and already correct on every path). The one core edit is a semantic
  fix to the `last_evicted` transient: `undo`/`redo` commit nothing, so they now
  reset it to 0 (see Component (a)).
- No new user-facing errors for over-cap auxiliary reads; the over-cap fallback is
  the existing safe degradation at each site (see below).
- No change to the authoritative load path (`file::open` is already bounded).

## Component (a) — undo-eviction hint via one universal main-loop check

**Revised twice after spec review.** The per-caller approach is whack-a-mole
(Codex found 7+ `Buffer::apply` sites; a future one would silently miss the hint).
An in-`reduce` seam does not work either: `reduce` (`app.rs:1091`) captures
`before` at `:1675` but has many EARLY RETURNS above it (search overlay `:1545`/`:1552`,
diagnostics quick-fix `:1602`, menu/palette dispatch), so those edit paths never
reach the `if version != before` block at `:1789`. The clean seam is OUTSIDE
`reduce`, at its single production call site in the run loop (`app.rs:2149`) — every
production edit funnels through exactly one `reduce(...)` call there.

- Single-source the hint text as a module-level const in `editor.rs`:
  `const UNDO_EVICTED_HINT: &str = "Undo history full — oldest dropped";` (the exact
  string `Editor::apply` uses today).
- Add a helper on `Editor`:
  ```
  /// Surface the undo-eviction hint iff an edit landed on the STILL-active buffer
  /// this reduce AND it evicted. Consumes `last_evicted` (resets to 0) so a later
  /// undo/redo/switch — which change `version` without a fresh eviction — do not
  /// re-fire the stale hint.
  pub fn note_undo_eviction(&mut self, pre_id: BufferId, pre_version: u64) {
      let fire = {
          let b = self.active();
          b.id == pre_id && b.document.version != pre_version
              && b.document.history.last_evicted > 0
      };
      if fire {
          self.status = UNDO_EVICTED_HINT.to_string();
          self.active_mut().document.history.last_evicted = 0;
      }
  }
  ```
- **Wire it at the single run-loop `reduce` call** (`app.rs:2149`): capture the active
  buffer's `(id, version)` immediately BEFORE `reduce`, call `note_undo_eviction(pre_id,
  pre_version)` immediately AFTER. It runs after `reduce` set any outcome status, so
  the hint OVERRIDES on eviction (matching today's `Editor::apply`). Being outside
  `reduce`, it covers EVERY reduce path — including all early-return command handlers
  (search-overlay, quick-fix, menu/palette) and job-result merges applied via the
  end-of-reduce drain — for the active buffer.
- **Remove `Editor::apply`'s inline hint** (`editor.rs:646`): now redundant (a
  keystroke edit bumps the active version → the run-loop check fires). `Editor::apply`
  becomes a plain delegator. Hint logic lives in exactly one place.
- **Core fix (2 lines): reset `last_evicted` in `History::undo`/`redo`** (`history.rs:99`/`:111`).
  `last_evicted` means "revisions dropped on the most recent commit"; `undo`/`redo`
  drop nothing, so it should be 0 afterward. Without this, an INACTIVE-buffer
  evicting edit (transform/paste/scratch to a non-active buffer — never consumed by
  the active-buffer check) leaves `last_evicted > 0`; a later switch to that buffer
  followed by an undo/redo (which bump `version` but do not call `evict_to`) would
  fire a spurious hint. Adding `self.last_evicted = 0;` makes the field honest and
  closes that residual false-positive at the root. **Placement:** insert it as the
  VERY FIRST statement of both `undo` and `redo`, BEFORE the `current == 0` /
  `current >= revisions.len()` early-return guards (`history.rs:100`/`:112`), so even
  a no-op undo/redo consumes stale eviction state. Breaks no core test
  (`eviction_keeps_current_consistent_for_undo_redo` asserts `last_evicted > 0`
  BEFORE its undo/redo, `history.rs:334`).

Rationale for override (not a combined message): uniform hint across all edit
paths, reuses one string, and eviction (>64 MiB undo history) is rare — the
undo-depth-loss warning is the point, and the filter/transform result is visible
on screen regardless.

**Why the `(id, version)` gate + reset (avoids false positives):**
- Same-`id` guard: a buffer SWITCH changes `active().id`, so it does not fire (the
  switched-to buffer's stale `last_evicted` was already consumed when it was active).
- `version != pre_version` gate: fires only when the active buffer was actually
  edited this reduce (version is monotonic per buffer).
- reset-to-0: the evicting commit's hint shows exactly once; a subsequent
  undo/redo on the active buffer sees `last_evicted == 0` (the shell consumed it, AND
  the core fix above zeroes it on undo/redo anyway) → no spurious re-fire.
  `last_evicted` is a per-commit transient (`evict_to` zeroes it at the start of every
  commit), so both the shell consuming it and the undo/redo reset are consistent.

**Known best-effort gaps (documented, acceptable for a cosmetic hint):**
- An edit AND a buffer-switch in the SAME reduce: the post-reduce active id differs
  from `pre_id` → no hint. Rare, cosmetic.
- Edits to an INACTIVE buffer (inactive transform/paste/scratch) do not surface the
  hint at edit time (they don't change `active().version`) — correct, that buffer is
  not on screen. With the core `undo`/`redo` reset, switching to such a buffer and
  undoing/redoing no longer fires a spurious hint either. The genuine eviction on
  that buffer simply isn't announced (invisible when it happened); acceptable.

## Component (b) — bound the three remaining reads

- Add a helper in `file.rs`:
  ```
  /// Read `path` fully, but cap the allocation at `limit` bytes. Returns `None` if
  /// the file exceeds `limit` OR any read/open error occurs — every caller treats
  /// `None` as its existing safe degradation (skip the optimization / assume differing).
  pub fn bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>> {
      let mut buf = Vec::new();
      let f = std::fs::File::open(path).ok()?;
      std::io::Read::read_to_end(&mut f.take(limit + 1), &mut buf).ok()?;
      if buf.len() as u64 > limit { return None; }
      Some(buf)
  }
  ```
  (Mirrors `file::open`'s `.take(limit + 1)` + `len > limit` check; the `+1` lets a
  file of exactly `limit` bytes succeed.)
- Apply the bound at each site. Two sites use `bounded_read_opt` directly (their
  `None` fallback is already safe); the fingerprint site needs a different shape
  (see below) to avoid a data-loss regression flagged in spec review:

| Site | Was | Over-cap handling |
|---|---|---|
| `app.rs:1936` recovery predicate | `std::fs::read(p).ok()` → `Option<Vec<u8>>` passed to `swap::assess` | `bounded_read_opt` → `None` → `assess` returns `RecoveryDecision::Prompt` (prompt the user — safe) |
| `file.rs:138` `save_atomic` skip-unchanged | `if let Ok(existing) = fs::read(path)` | `bounded_read_opt` → `None` → skip the skip-unchanged optimization → proceed to the atomic write (safe) |
| `save.rs:31` `fingerprint()` | `std::fs::read(path).ok()?` (full-file content hash) | metadata-based fallback — see below (NOT a bare `None`) |

The recovery + skip-unchanged sites are cold paths whose `None` (over-cap or
read-fail) already takes the safe branch — no user-facing error.

### `fingerprint()` over-cap: metadata fallback (not `None`)

**Spec-review finding (Important):** a bare `bounded_read_opt → None` here
reintroduces a BUG-2-class silent-overwrite for over-cap files. `fingerprint()`
is called BOTH for the pre-save conflict check (`current_fp != stored_fp`) AND to
STORE `stored_fp` after a successful save (`save.rs:70`/`:87`). If an over-cap file
made `fingerprint()` return `None`, then `stored_fp` becomes `None`, and the next
save computes `current_fp == None` too — `None != None` is FALSE, so the
external-mod modal never fires and external changes are silently overwritten.

**Fix:** `fingerprint()` must never return `None` for a *present, readable* file
merely because it is over-cap. Compute the content hash over a bounded read; on
over-cap, fall back to a metadata-only fingerprint (real `mtime` + `size`, with the
content `hash` set to a fixed sentinel, e.g. `0`). Shape:
```
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;                 // None only if truly unreadable/missing
    let limit = crate::limits::MAX_OPEN_BYTES;
    let hash = match crate::file::bounded_read_opt(path, limit) {
        Some(bytes) => { /* DefaultHasher over bytes, as today */ }
        None => 0, // over-cap (or transient read failure): sentinel — fall back to mtime+size
    };
    Some(FileFingerprint { mtime: meta.modified().ok(), size: meta.len(), hash })
}
```
Then over-cap files are still compared by `mtime + size` (catching truncation,
growth, and mtime changes — the common external mods); only the same-mtime,
same-size, different-content tiebreak (BUG-2's within-one-tick hash guard) is
unavailable for >64 MiB files — a narrow, documented narrowing, and NOT the
silent `None == None` defeat. Note `size` now comes from `meta.len()` (was
`bytes.len()`), which is correct for both branches. Plan-confirm the exact
`DefaultHasher` usage matches today's `fingerprint()` for the ≤cap path so normal
files hash identically (no fingerprint churn).

## Testing

- **(a):** `last_evicted` is a public `usize` field on `History`, so a test drives
  `note_undo_eviction(pre_id, pre_version)` directly (no 64 MiB edit needed). Build an
  `Editor` (`Editor::new_from_text`); record `pre_id = active().id`,
  `pre_v = active().document.version`; simulate an evicting edit by
  `active_mut().document.version += 1` and `active_mut().document.history.last_evicted = 1`;
  call `note_undo_eviction(pre_id, pre_v)` → assert `status == UNDO_EVICTED_HINT` AND
  `active().document.history.last_evicted == 0` (consumed). Negative/false-positive
  cases: (i) `version` unchanged (no edit) → no hint; (ii) `last_evicted == 0` → no
  hint; (iii) a second call after the reset → no re-fire (proves undo/switch won't
  replay the stale hint). Update/relocate the existing
  `apply_does_not_set_hint_when_no_eviction` (`editor.rs:1079`) to target the helper
  (its assertion — no hint without eviction — still holds; `Editor::apply` no longer
  carries the check).
- **(a) core reset:** add a `wordcartel-core` test that after an evicting commit
  (`evict_to` → `last_evicted > 0`), a subsequent `undo` resets `last_evicted == 0`,
  and likewise `redo` — proving the field is honest after non-committing operations.
- **(b) `bounded_read_opt`:** takes `limit` as a param → test with a TINY limit (no
  large file): write a 3-byte file with `limit = 4` → `Some`; write a 10-byte file
  with `limit = 4` → `None`; a missing path → `None`.
- **(b) `fingerprint()` over-cap:** since `fingerprint()` hardcodes `MAX_OPEN_BYTES`,
  make its bounded read testable — either extract an inner `fingerprint_with_limit(path, limit)`
  the public fn delegates to, or accept that only the ≤cap path is unit-tested and
  the over-cap metadata fallback is covered by review. With an injectable limit:
  write a 10-byte file with `limit = 4` → returns `Some` (metadata fallback:
  `mtime`/`size` populated, `hash == 0`), NOT `None`; a ≤cap file → `Some` with a
  real content hash identical to today's. Regression: existing `fingerprint_*` and
  `save_same_content_returns_unchanged` tests still pass on normal files.

## Decomposition

- **Task 1 — (a):** the core `last_evicted = 0` reset in `History::undo`/`redo` +
  its core test; `UNDO_EVICTED_HINT` const + `Editor::note_undo_eviction(pre_id,
  pre_version)` helper (with the `last_evicted` reset) + remove `Editor::apply`'s
  inline hint + capture `(id, version)` before the run-loop `reduce` call (`app.rs:2149`)
  and call the helper after + shell tests.
- **Task 2 — (b):** `file::bounded_read_opt` helper + convert the recovery-predicate
  and skip-unchanged sites (direct `None`) + convert `fingerprint()` to the
  metadata-fallback shape + tests.

The clippy deny-gate is already live, so each task keeps `cargo clippy --workspace
--all-targets` clean; no separate gate task.

## Plan-confirms (resolve during the implementation plan, against real source)

1. Confirm the run-loop `reduce` call at `app.rs:2149` is the SOLE production reduce
   call (the others are tests), and that capturing `(active().id, active().document.version)`
   just before it + calling `note_undo_eviction` just after it runs after `reduce`
   returns (so it overrides the outcome status) and before the draw. `editor` is a
   plain local there (mutable), so `active()`/`active_mut()` are in scope.
2. `Buffer.id` type/accessor (`BufferId`) + `Document.version` (`u64`) for the helper
   signature; where `UNDO_EVICTED_HINT` best lives (module-level const in `editor.rs`).
3. Confirm `swap::assess`'s `Prompt` fallback on `current_file_bytes == None`
   (`swap.rs:264`) so the recovery over-cap fallback is genuinely safe.
4. That `bounded_read_opt` belongs in `file.rs` (it's IO + already imports
   `limits`) and is reachable from `save.rs`/`app.rs` (`crate::file::bounded_read_opt`).
5. The `fingerprint()` ≤cap path must hash identically to today (same
   `DefaultHasher` sequence) so normal files produce no fingerprint churn; confirm
   `FileFingerprint.size` semantics are unchanged when switching from `bytes.len()`
   to `meta.len()` (equal for a ≤cap file read fully).
