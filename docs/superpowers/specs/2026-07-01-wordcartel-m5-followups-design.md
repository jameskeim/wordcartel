# M5 follow-ups: undo-eviction hint + bound the last document-sized reads — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-01
**Effort:** M5 follow-ups (pre-Effort-P; two small M5 leftovers, one bundled effort)

## Context

M5 (resource caps) left two documented loose ends. This effort closes both. They
share no code but are both small "finish M5" cleanups, bundled into one branch to
amortize the gated-pipeline overhead. **Shell-only; no `wordcartel-core` changes.**

- **(a) Undo-eviction hint is incomplete.** M5 bounds undo history by BYTES
  (`MAX_UNDO_BYTES = 64 MiB`, `history.rs:9`): when the retained revisions exceed
  the budget, `History::evict_to` (`history.rs:68`) drops the oldest and records
  the count in `History.last_evicted` (`history.rs:56`). Both `History::commit`
  and `commit_coalescing` call `evict_to`, so eviction is **always enforced in
  core, on every edit path** — this effort does NOT change that. The *hint* to the
  user, however, is surfaced only on the command/keystroke path (`Editor::apply`,
  `editor.rs:646`), which reads `last_evicted` and sets
  `status = "Undo history full — oldest dropped"`. The large buffer-level merges
  (filter, transform, paste) go through `Buffer::apply` directly and set their own
  outcome status, so they never surface the hint. Purely cosmetic gap.
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

- Surface the undo-eviction hint uniformly on ALL edit paths (keystroke AND the
  filter/transform/paste buffer-merges), single-sourcing the hint text.
- Bound the three remaining document-sized reads at the `MAX_OPEN_BYTES` cap, each
  falling back to its existing safe degradation on over-cap (no user-facing error).
- No behavior change beyond the hint surfacing + the allocation cap; no core changes.

## Non-goals

- No change to the eviction mechanism itself (core `evict_to` is unchanged and
  already correct on every path) — this is hint surfacing only.
- No new user-facing errors for over-cap auxiliary reads; the over-cap fallback is
  the existing safe degradation at each site (see below).
- No change to the authoritative load path (`file::open` is already bounded).

## Component (a) — undo-eviction hint on the buffer-merge paths

- Single-source the hint text as a const (shell side, near `Editor`):
  `const UNDO_EVICTED_HINT: &str = "Undo history full — oldest dropped";` (the exact
  string `Editor::apply` uses today).
- Add a helper `Editor::note_undo_eviction(&mut self, buffer_id: BufferId)`:
  ```
  if self.by_id(buffer_id).map_or(false, |b| b.document.history.last_evicted > 0) {
      self.status = UNDO_EVICTED_HINT.to_string();
  }
  ```
- **Refactor `Editor::apply`** (`editor.rs:646`) to call `self.note_undo_eviction(self.active().id)` instead of its inline check + literal, so the hint text lives in exactly one place (behavior-identical).
- **Wire the helper at the three buffer-merge sites, AFTER each outcome status is set** (so the hint overrides on eviction — matching `Editor::apply`, which replaces status with the bare hint):
  - filter merge — after `editor.status = "filter applied".into()` (`app.rs:327`): `editor.note_undo_eviction(buffer_id);`
  - transform merge — after `editor.status = kind.past_tense().to_string()` (`transform.rs:180`): `editor.note_undo_eviction(buffer_id);`
  - paste — after the paste's outcome status is set (the caller of the `Buffer::apply` at `app.rs:783`): `editor.note_undo_eviction(buffer_id);` (plan-confirm the exact paste status site + that `buffer_id` is in scope there).

Rationale for override (not a combined message): uniform hint across all edit
paths, reuses one string, and eviction (>64 MiB undo history) is rare — the
undo-depth-loss warning is the point, and the filter/transform result is visible
on screen regardless.

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
- Apply `bounded_read_opt(path, crate::limits::MAX_OPEN_BYTES)` at each site, each
  landing on its ALREADY-safe fallback:

| Site | Was | Over-cap fallback (unchanged semantics) |
|---|---|---|
| `app.rs:1936` recovery predicate | `std::fs::read(p).ok()` → `Option<Vec<u8>>` passed to `swap::assess` | `None` → `assess` returns `RecoveryDecision::Prompt` (prompt the user — safe) |
| `save.rs:31` `fingerprint()` | `std::fs::read(path).ok()?` | `None` → no fingerprint → `!= stored_fp` → conservative external-mod modal (safe) |
| `file.rs:138` `save_atomic` skip-unchanged | `if let Ok(existing) = fs::read(path)` | `None` → skip the skip-unchanged optimization → proceed to the atomic write (safe) |

All three are cold paths (startup recovery / save / save). None ever surface an
error to the user on over-cap — they just lose an optimization or take the
conservative branch, which is the same thing that happens today when the read
fails for any other reason.

## Testing

- **(a):** `last_evicted` is a public `usize` field on `History`, so a test can set
  it directly (no 64 MiB edit needed): build an `Editor` (`Editor::new_from_text`),
  set `active_mut().document.history.last_evicted = 1`, call
  `note_undo_eviction(active_id)`, assert `status == UNDO_EVICTED_HINT`; negative
  case (`last_evicted == 0` → status untouched). The existing
  `apply_does_not_set_hint_when_no_eviction` (`editor.rs:1079`) stays green (now
  routed through the helper).
- **(b):** `bounded_read_opt` takes `limit` as a param → test with a TINY limit (no
  large file): write a 3-byte file with `limit = 4` → `Some`; write a 10-byte file
  with `limit = 4` → `None`; a missing path → `None`. Regression: the existing
  `fingerprint_*` and `save_same_content_returns_unchanged` tests still pass on
  normal small files (the helper's normal-size path is byte-identical to the old
  `fs::read`).

## Decomposition

- **Task 1 — (a):** `UNDO_EVICTED_HINT` const + `Editor::note_undo_eviction` helper +
  refactor `Editor::apply` to use it + wire at the 3 buffer-merge sites + tests.
- **Task 2 — (b):** `file::bounded_read_opt` helper + convert the 3 read sites +
  tests.

The clippy deny-gate is already live, so each task keeps `cargo clippy --workspace
--all-targets` clean; no separate gate task.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact paste outcome-status site (the caller of the `Buffer::apply` at
   `app.rs:783`) where `note_undo_eviction(buffer_id)` should land, and that
   `buffer_id` is in scope there (the map noted paste sets status at the caller
   level, not inside `insert_paste_text`).
2. `Editor::by_id`/`by_id_mut` + `Buffer.id` accessor names/visibility for the
   helper; where `UNDO_EVICTED_HINT` best lives (module-level const in `editor.rs`).
3. Confirm `swap::assess`'s `Prompt` fallback on `current_file_bytes == None`
   (`swap.rs:264`) so the recovery over-cap fallback is genuinely safe.
4. That `bounded_read_opt` belongs in `file.rs` (it's IO + already imports
   `limits`) and is reachable from `save.rs`/`app.rs` (`crate::file::bounded_read_opt`).
