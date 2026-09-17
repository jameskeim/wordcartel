# Fable whole-change review — recovery safety (R1 / R4 / R5)

Date: 2026-09-14. Reviewer: Claude Fable 5.1 (independent final gate).
Snapshot: uncommitted working tree on `fix/recovery-safety`, base `main` 9e409c0.
Source identity: all 44 files in `docs/reviews/evidence/2026-09-14-recovery-implementation/source-sha256.json`
re-hashed in this checkout, 0 mismatches. Authority: design revision 5, plan revision 2,
implementation ledger.

## Verdict: NO-GO (one gate red; nothing in the data-safety core is broken)

The durability core is sound: I found no data-loss or panic path in ownership, strict
acknowledgement, the CAS-guarded source retirement, the same-handle save receipt, legacy
copy-only import, cancellation, or quit barriers. The NO-GO is driven by a hard merge gate
that fails on this exact snapshot, plus four behavioural findings that are real, probe-confirmed,
and user-visible on common paths. Nothing here needs a redesign; every item is a bounded fix
or an explicit human decision.

| Gate (CLAUDE.md) | Result here | Note |
| --- | --- | --- |
| `cargo clippy --workspace --all-targets` | **FAIL** — 4 × `clippy::module_inception` | C-1 below. The evidence `clippy.log` is a bare dev-profile `cargo clippy`; it never compiled test targets. |
| `cargo test --workspace` | green (no-fail-fast run: every target ok) but **2 of 5 lib-suite runs had 1 intermittent failure** | I-5 below. |
| `cargo build`, `cargo test --no-run` | warning-free | |
| `git diff --check` | clean | |
| `scripts/smoke/run.sh` (advisory) | `smoke: 9/9 PASS` | quoted verbatim; run in a private tmux server. |
| module budgets / backlog / fs_chokepoint / command-surface invariants | pass | |

Probe commands and raw results: `review-probes/` (`probes.rs`, `01-probes.log`, `02-gates.log`,
`03-flake-reruns.log`). The probe module was compiled against the real branch through a temporary
`mod` line in `lib.rs`; `lib.rs` was restored and its sha256 re-verified against the manifest.

---

## Critical

### C-1 — Workspace clippy gate fails: `module_inception` in four new test files
- **Symbols:** `recovery_flow/import/tests.rs:2`, `recovery_flow/discovery/tests.rs:2`,
  `recovery_flow/pending/tests.rs:2`, `recovery_picker/tests.rs:2` — each is loaded as
  `mod tests;` and then declares an inner `#[cfg(test)] mod tests { … }` (`tests::tests`).
- **Trigger:** `cargo clippy --workspace --all-targets` (the gate command) →
  `error: module has the same name as its containing module`, `-D clippy::module-inception`
  implied by the workspace `clippy::all = "deny"`; `could not compile wordcartel (lib test)`.
- **Evidence:** `review-probes/02-gates.log`. The recorded `clippy.log` shows only
  `Checking wordcartel … Finished dev profile` — a check of the non-test targets, so the ledger's
  "workspace clippy clean" claims were not produced by the gate command.
- **Why it exists:** the round-1 workspace log shows `fs_chokepoint` failing on raw `std::fs`
  calls in exactly these files; the fix was to wrap each file in `#[cfg(test)] mod tests {}` so
  the chokepoint scanner's `strip_test_modules` skips them. That wrapper is what trips clippy.
- **Fix:** drop the inner wrapper and keep the file itself as the test module (the parent already
  declares `#[cfg(test)] mod tests;`), moving the `use super::super::*` to `use super::*` at file
  top. Re-verify BOTH `cargo clippy --workspace --all-targets` and `cargo test -p wordcartel
  --test fs_chokepoint` — the scanner only recognises the `#[cfg(test)]` + `mod tests` pair, so
  the chokepoint gate must be re-run after the change (an `EXEMPT_MODULES` row with a clause is
  the other legitimate route). Then re-record `clippy.log` from the gate command.

## Important

### I-1 — The 5 s "pending IO" timeout is applied to ordinary idle checkpoints, outside quit, and discards a successful Ack
- **Symbols:** `recovery_flow::pending::pending_deadline` (counts every non-detached entry in
  `RecoveryState.requests`, not just `association` ones), `pending::timeout_tick` (sets
  `cancelled = true` on them), `recovery_flow::complete` (`if request.cancelled { return; }`
  before recording the Ack), `import::expire` (same for handoffs; also `cancel_batch`),
  `timers::pre_recv` (unconditional).
- **Trigger:** any checkpoint whose worker execution exceeds 5 s, with NO quit in progress. A
  first checkpoint fsyncs the record, the owner dir and every ancestor to `/` after writing up to
  64 MiB; on an HDD, a busy machine, or a network home directory this is plausible.
- **Effect (probe P1, confirmed):** sticky warning "Recovery IO is still pending" appears with no
  quit; when the worker then succeeds the Ack is discarded (`recovery_ack == None`,
  `swapped_version == None`) although the record IS on disk; `timers::next_wake` immediately
  returns `Some(now)` and the buffer re-checkpoints identical content. Every such cycle repeats
  the warning. For a multi-select import the same path cancels the rest of the batch and CAS-cancels
  a retirement that would have been safe. This contradicts the design's own scope for the
  timeout ("bounds foreground waiting state", "cancel current foreground quit once") and the
  resource rule "proportional to work, free at rest".
- **Fix:** (a) arm the deadline only for exit-blocking work (`association`/handoff requests) and
  only while `quit::in_progress`; (b) on timeout set `detached` (drop the exit blocker) but do
  NOT set `cancelled` on ordinary/association checkpoints — a late Ack must still latch
  (`swapped_version`, `recovery_ack`); (c) for handoffs keep `progress.cancel()` only when quit
  is in progress (design F row 8). Add a regression: ordinary checkpoint completing at t=6 s with
  no quit → Ack recorded, no warning, no re-dispatch.

### I-2 — The D2 happy path (recover → first Save As, clean) leaves a checkpoint that is re-offered on every later launch, and the save reports a warning
- **Symbols:** `recovery_store::cleanup::cleanup_saved` (association must equal the committed
  destination; a handoff successor has `association == None`), `save::merge_save` →
  `recovery_flow::association_changed` (returns early when `!dirty`, so a clean Save As queues no
  association checkpoint), `recovery_flow::discovery::offer` (dismissals are per-session; the
  bootstrap all-candidate scan re-offers the record next launch).
- **Trigger (probe P2, confirmed):** import a candidate, Save As without further edits. Result:
  status `Saved to … — Recovery checkpoint retained: checkpoint is not covered by this save`
  (kind Warning) on a fully successful save; the successor record stays under `recovery-v2/`,
  is not busy after exit, and the next launch's bootstrap scan pops the picker with it. The same
  shape occurs for a clean ordinary Save As A→B (probe P6; the branch's own test
  `recovery_save_as_clean_rekey_preserves_owner_and_reports_retained_old_association` pins it).
  `Clean Recovery Files` cannot remove it (all of `recovery-v2` is protected), so the only user
  remedy is to edit the document, wait for a checkpoint, and save again — not discoverable.
- **Design status:** conformant with design §C as written ("association is the committed
  destination"; "clean state … creates no old-body checkpoint"). So this is a **design gap the
  human must rule on**, not an implementation deviation. Two bounded options:
  (1) on an accepted Save As whose write completed with `version == v`, still enqueue one
  association checkpoint (current snapshot, new association) so the next save's receipt can
  retire it — one extra write per Save As; or (2) let the receipt accept a record whose
  association is `None` or the pre-rekey path when it is a SaveAs completion and the bytes are
  exactly covered (the saved bytes are durably on the committed target, so retirement cannot
  lose data). Either way the Save-As status should not be a Warning when the retained copy is a
  byte-identical successor of a successful save.

### I-3 — One unreadable/corrupt entry in the state dir makes the recovery picker open on every contextual file open, forever
- **Symbols:** `recovery_discovery::scan` — the `ScanScope::Associated` filter keeps
  `(!row.busy && row.unavailable.is_some())` rows regardless of association; `discovery::offer`
  — rows with `token == None` pass the dismissed/opened filters (`is_none_or`), so
  `result.is_ok_and(Vec::is_empty)` is false and a picker is shown; `recovery_picker::close`
  can only record tokens, so a token-less row is never dismissable.
- **Trigger (probe P3, confirmed):** a single `junk.swp` with an invalid header (also: a
  non-UTF-8 legacy body, an oversized legacy header, an owner dir left with only a `.tmp` after a
  crash mid-write, a `recovery-v2` whose mode drifted from 0700 — probe P5). Opening A shows the
  picker with only an unavailable row; Esc; opening B shows it again; reopening A again. The
  legacy cleaner offers nothing (fail-closed), and `recovery-v2` is fully protected, so there is no
  in-app remedy. The design wants corrupt entries "visible", but also says "dismissal alone must
  not cause repeated interruptions".
- **Fix:** in `scan`'s Associated filter, keep only error rows that describe the *scan itself*
  (root/protocol-level errors) and drop per-entry unavailable rows from *automatic* offers (manual
  review still lists them); or give unavailable rows a dismissable identity (path + mtime + len)
  and record it on close. Add a regression: corrupt entry + two contextual opens → at most one
  automatic offer per session.

### I-4 — Permanent checkpoint failure retries with a zero-delay wake (hot loop); the new strict sequence adds new permanent-failure classes
- **Symbols:** `recovery_flow::complete` (Err path leaves `last_swap_at`/`swapped_version`
  untouched), `timers::swap_deadline` → `swap::next_deadline_ms` (`max(now, …)` of an already
  past instant → `Some(now)`), `recovery_store::checkpoint_with_names` (first-Ack ancestor
  `sync_dir_strict` chain to `/`; `allocate` → `validate_private_dir` on `recovery-v2`).
- **Trigger (probe P4, confirmed):** with `sync_dir_strict` failing permanently, 50 loop
  iterations produce 50 checkpoint dispatches and 50 `next_wake == Some(now)` — i.e.
  `recv_timeout(0)` → Tick → dispatch → fail → repeat, bounded only by the failing IO's latency.
  New ways to be permanently failing that HEAD did not have: a directory `fsync` refused by the
  filesystem anywhere on the ancestor chain (some FUSE/network mounts), or `recovery-v2`
  ownership/mode not matching (`validate_private_dir`) — probe P5 shows the mode is never
  re-established (HEAD's `state_dir()` re-applied 0700 on every call).
- **Status:** the spin itself is pre-existing (HEAD's `dispatch_swap_write` merge behaved the
  same on a failed write) and the design defers ordinary-failure retry policy to R3. But the
  branch widens the trigger set, so the human should decide whether R3 must land first.
  Minimal mitigation inside this effort: after a checkpoint failure set `last_swap_at =
  Some(now)` (or a dedicated backoff stamp) so `due()`'s max-cap path governs the retry (30 s),
  and re-apply 0700 to `recovery-v2` in `allocate` before validating (it is protocol-owned).

### I-5 — Intermittent `cargo test` failure: `recovery_discovery_changed_v2_requires_reselection_and_releases_lease`
- **Symbols:** `recovery_regressions.rs:249` — `RealFs.try_recovery_lock(&lock, false).unwrap()`
  immediately after `scan()` released the same lock.
- **Trigger:** parallel test execution while another test spawns a subprocess
  (`recovery_process_tests`, `fifo_open_returns_without_waiting_for_a_writer`): the child inherits
  the flock'd descriptor between `fork` and `exec` (closed at exec by CLOEXEC), so the lock is
  transiently busy. Observed 2 red results in 5 full lib-suite runs here (one attributed to this
  test with `WouldBlock: recovery record is active`; the other run's failing name was not
  captured — see `review-probes/03-flake-reruns.log`); passes in isolation. The ledger already
  added a bounded retry for this window in `Fixture::new` and `assert_released`, but not at this
  site.
- **Fix:** use the same bounded-retry helper (`assert_released`-style loop) for the acquisition at
  line 249, or run the subprocess-spawning tests with a shared lock so they never overlap lock
  assertions. A flaky merge gate is a red gate.

### I-6 — Evidence integrity: the recorded gate logs do not show the gates passing on this snapshot
- `clippy.log` / `task*-clippy.log`: dev-profile `cargo clippy` only (see C-1).
- `workspace-tests.log` (the "final rerun") is truncated at 2157 lines with no `test result`
  summary; the only complete unrestricted run on file (`workspace-tests-round1.log`) has
  `fs_chokepoint` FAILED. The chokepoint failure is fixed in this snapshot (my run: 10/10), but
  the fix is what broke clippy, and no log records both gates green together.
- **Fix:** re-run the six commands in the plan's "Final coverage" block on the final snapshot and
  replace the logs; the ledger's "workspace clippy clean" rows for Tasks 1–7 should be corrected.

## Minor

### M-1 — `recovery_picker::close` / `discovery::review` call `cancel_scans`, which also discards other buffers' queued contextual assessments
`cancel_scans` clears `scans.queue` and `scans.requests`, not just the scan that fed the picker.
Practically moot today (an all-candidate picker has already displayed those tokens, and its
dismissal suppresses them), but a contextual picker for Y closing while X's intent is queued
silently drops X's assessment for the session. Consider clearing only offers for the closing
picker and letting queued intents run.

### M-2 — Tombstone and crashed-temp owner directories accumulate without any reclamation path
Every buffer that ever checkpoints leaves `recovery-v2/<owner>/owner.lock` forever (design
defers reclamation), and each costs ~5 syscalls per scan (validate, lock, open, list). A crash
between temp write and rename leaves a permanent *unavailable* row (see I-3) with no cleaner.
Design-governed; worth a backlog item with a bound (e.g. reclaim lock-only dirs older than N days
under the exclusive lock).

### M-3 — Duplicate helpers
`recovery_discovery::validate_owner` duplicates `recovery_store::codec::validate_owner`;
`recovery_discovery::normalized` duplicates `recovery_flow::resolve_association` (both walk
missing suffixes over `canonicalize_existing`). One home each.

### M-4 — `scan` clones every legacy/v2 body once more than needed
`read_v2` / `read_legacy` build a `PreparedRecovery` (`body.to_owned()`) that `scan` immediately
discards, so each candidate transiently holds two copies of up to 64 MiB on the worker. Return
`(Candidate, Option<lease>)` from a scan-specific path, or keep `PreparedRecovery` only in
`prepare`.

### M-5 — House-style deviations in the new flow modules
`recovery_flow/import.rs`, `discovery.rs`, `pending.rs`, `recovery_picker.rs` pack several
statements per line and 150–200-column lines (e.g. `import.rs` `ImportRequest`, `install`,
`finish`); the surrounding code is dense but hand-wrapped at ~100 columns. Review-only; no
functional impact.

### M-6 — Sticky Warning on successful save whenever a retained checkpoint is expected
Related to I-2: "Recovery checkpoint retained: …" is emitted as a Warning-kind completion for
outcomes that are normal (Save As, save while a newer generation exists). Consider Info for the
"retained by rule" cases and Warning only for IO faults.

---

## What was verified as correct (no finding)

- **Ownership (R1/D4):** exclusive `create_dir_excl` allocation with 128-attempt collision
  bound, lock created before any record is published, stable lock inode never unlinked, slot
  identity by `Arc::ptr_eq`, jobs clone the slot so leases outlive Buffer drop (subprocess test
  `queued` mode + my reading of `Owner`/`SlotState`). Same-path and unnamed buffers never share a
  directory.
- **Strict acknowledgement:** `checkpoint_with_names` consumes the attempted generation before
  IO, allocates, writes temp 0600 → flush → `sync_all` → rename → `sync_dir_strict(owner)` →
  (until first Ack) every ancestor to `/`; Ack only after the full sequence; failure keeps
  `acknowledged` unchanged so the whole chain is re-owed on retry, including after a poisoned
  mutex. Replayed/unreserved generations are refused.
- **Source retirement (R5/D5):** handoff job writes the successor with the captured
  pre-callback body, `record_ack`, then `authorize()` (durable flag + CAS Pending→Retiring) and
  only then `remove_file(source)` + strict parent sync, all in one FIFO worker operation holding
  the source lease; legacy and retry paths never unlink. Cancel-before-CAS keeps the source;
  cancel-after cannot revoke. Late merges compare BufferId + slot identity and generation.
- **Save receipt:** `cleanup_saved` under the owner guard, reads the owned record via
  `open_regular_nofollow`, demands owner/generation ceiling/edit-version/body/association
  equality, then `prove_saved` opens the committed target ONCE, compares full bytes, same-handle
  fingerprint, `sync_all` on that handle, strict parent sync, and consumes a non-Clone receipt.
  Fault matrix (open/read/stat/sync/dir-open/dir-sync/unlink/canonicalize) retains the record;
  failures never demote a successful save. `panicx::catch` isolates cleanup panics.
- **Legacy preservation:** `.swp` and `recovered-*.md` are read by header, never renamed or
  unlinked anywhere in scan/prepare/handoff; symlinks/FIFOs rejected without blocking
  (`O_NOFOLLOW|O_NONBLOCK`, metadata validated on the opened handle).
- **Cancellation/quit:** `has_pending_work` blocks only concrete association/handoff work;
  `after_job` runs before quit re-drive in both wrappers; `after_callbacks` runs before the
  `!editor.quit` early return; quit-owned Save As continuation is released on cancel; prepared
  results arriving during quit are not installed.
- **Selector/command:** `review_recovery_files` registered in File category via the registry;
  overlay row in `OVERLAYS`/`ALL`/`RENDER_ORDER` with one geometry for paint and hit-test;
  zero-selection stays open; Esc/click-away/registry replacement share `close`; rendered
  tiny-terminal tests pass.
- **Opening flows (R4/D6):** `opened()` is called after successful install in both
  `workspace::open_as_new_buffer` and `session_restore::open_into_current`; bootstrap runs after
  executor/wake-relay creation and before the first receive; discovery is worker-only and
  edge-triggered (no idle scans).
- **Resource/IO boundaries:** no filesystem IO in reduce/render/timers; scans one record at a
  time with capped reads; previews bounded to 240 scalars; `MAX_RECORD_BYTES` consistent with
  codec caps.
- **Regression vs HEAD:** all pre-existing durability/quit tests pass; `SwapWrite` migration
  complete (no live old-format writer; `rg` census clean); panic-dump path untouched.

## Limitations

- Linux only; Windows durability (`FlushFileBuffers` on `BACKUP_SEMANTICS` directory handles,
  reparse-point rejection) is compiled-for but unvalidated here, as the brief states.
- Process-death tests and probes exercise ordering and lock release; none simulate power loss.
- `ThreadExecutor::drop` still joins the worker; the 5 s timeout bounds foreground waiting only.
- Lua event hooks are observer-only; the recovered-Open contract was exercised through registered
  commands, as the ledger records.
- The hot-loop probe (P4) uses an instantly failing `FaultFs`; real failing IO would pace the loop
  by its own latency.
- This review compiled probes against private APIs through a temporary `lib.rs` line; the tree
  was restored and hash-verified. `scripts/smoke/.history` (gitignored) gained one line from the
  mandated smoke run.
