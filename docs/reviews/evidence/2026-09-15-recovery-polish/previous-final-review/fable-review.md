# Fable whole-change review (round 2) — recovery safety (R1 / R4 / R5)

Date: 2026-09-15. Reviewer: Claude Fable (independent final gate, second round).
Snapshot: uncommitted working tree on `fix/recovery-safety`, base `main` `9e409c0`.
Source identity: all 45 files in
`docs/reviews/evidence/2026-09-14-recovery-implementation/source-sha256.json` re-hashed in
this checkout — 0 mismatches (`review-probes/verify_hashes.py`). Authority: design revision 5
plus the 2026-09-15 user-approved amendments, plan revision 2, implementation ledger, and the
round-1 triage dispositions.

## Verdict: GO

Every merge gate passes on this exact snapshot. The four round-1 Important findings, the
round-1 Critical clippy gate, and the flaky-test and evidence findings are fixed, and I
verified each against the revised source and by running the gates myself. The durability core
holds: I found no data-loss path and no panic path in ownership, strict acknowledgement, the
CAS-guarded source retirement, the same-handle save receipt, legacy copy-only import,
cancellation, or the quit barriers. The remaining items are Minor — each is either
design-conformant or an already-documented deferral, and none loses data.

## Gate results (run in this checkout)

| Gate (CLAUDE.md) | Result | Note |
| --- | --- | --- |
| `cargo clippy --workspace --all-targets` | PASS, clean | round-1 C-1 (`module_inception`) is gone; test files use distinct `#[path]` module names |
| `cargo test --workspace` | green | lib 2181 passed / 2 ignored; all other suites green; ran the lib suite and workspace repeatedly with no failure |
| `cargo build --workspace` | warning-free | |
| `cargo test --workspace --no-run` | warning-free | |
| `git diff --check` | clean | |
| `scripts/smoke/run.sh` | `smoke: 9/9 PASS` | quoted verbatim; private tmux server |
| snapshot manifest re-hash | 45/45 match | |
| `validation.json` integrity | consistent | records the six final commands including `cargo clippy --offline --workspace --all-targets` exit 0; recorded `clippy-final.log`/`workspace-tests.log` sha256 match the files in the evidence dir |

The round-1 evidence-integrity finding (I-6) is resolved: unlike the first snapshot, the
recorded final logs are the `--workspace --all-targets` runs, their hashes match `validation.json`,
and I independently reproduced clippy-clean and a green workspace on the same source.

## Round-1 findings — verification

- **C-1 (clippy gate red):** FIXED. `cargo clippy --workspace --all-targets` is clean here; the
  new test files carry named module paths, so `module_inception` no longer fires, and
  `fs_chokepoint` stays green (10/10 in the workspace run).
- **I-1 (5 s timeout applied to idle checkpoints, lost Ack):** FIXED.
  `recovery_flow::pending::pending_deadline` and `timeout_tick` both early-return unless
  `quit::in_progress`. A background checkpoint that runs long neither warns nor drops its Ack
  (regressions `recovery_background_slow_checkpoint_latches_without_warning_or_retry`,
  `recovery_background_slow_handoff_does_not_cancel_batch_or_retirement`).
- **I-2 (clean Save As warning + re-offer):** FIXED for the case the finding covered.
  `cleanup_saved_with_policy` with `AssociationPolicy::SaveAs` retires the owned successor when
  its exact bytes are proven durable at the new destination, reporting Info, and
  `CleanupOutcome::RetainedByRule` separates expected retention from IO faults (regressions
  `recovery_handoff_first_save_as_retires_successor_without_warning`,
  `recovery_save_as_clean_rekey_retires_exact_owned_checkpoint`). This is the user-approved
  amendment. Probe P3 shows the narrower residual (failed import then Save As) under Minor M-B.
- **I-3 (corrupt entry re-offered forever):** FIXED. Token-less unavailable rows get a
  per-session dismissal identity (`discovery::UnavailableIdentity`: path + displayed time +
  reason); manual review still lists them. Probe-confirmed via
  `recovery_picker_corrupt_row_dismisses_across_contexts_but_manual_remains_visible`.
- **I-4 (zero-delay retry hot loop):** FIXED. `recovery_flow::retry::RetryState` arms a 30 s
  floor at the foreground completion clock; `timers::swap_deadline` folds it via `constrain`
  and `on_tick` gates on `recovery_retry.ready`. Queue rejection and panic share the path
  (`recovery_retry_*` regressions). The mode-drift permanent-failure class is now paced by the
  same floor rather than spinning; not auto-repairing the directory mode is the explicit
  user decision, not a defect.
- **I-5 (flaky lock-acquisition test):** FIXED at the flagged site — no raw
  `try_recovery_lock(..).unwrap()` remains in the tree; released-lease assertions go through the
  bounded `released_lock` / `recovery_scan_released` / `recovery_prepare_released` helpers. I ran
  the lib suite (which includes the subprocess-spawning tests that triggered the race) and the
  full workspace multiple times with zero failures. I cannot prove absence of a rare race, but
  could not reproduce it.

## Findings (this round)

### Minor

**M-A — Esc during the picker's Opening phase, after a recovered buffer is installed, leaves a
durable successor the buffer does not track.**
Symbols: `recovery_picker::close` → `recovery_flow::cancel_batch`; `import::finish` (the
`if !request.cancelled` latch guard and the `request.cancelled || request.detached` early
return); `import::install`. Trigger (probe P2, confirmed): select a v2 candidate, and press Esc
in the window after the handoff is dispatched but before it merges. The worker writes the
successor checkpoint, then loses the authorization CAS to the cancel, so the source is retained
(safe). But because the request is cancelled, the successful Ack is never latched: the recovered
buffer ends dirty with `recovery_ack == None`, no `recovery_protection_failure`, and — because a
recovered install never arms `last_edit_at` — `timers::next_wake` returns `None`, so no fresh
checkpoint is scheduled until the user edits. No data loss: both the source and a complete
successor `.wcr` are on disk, and the successor is rediscovered next launch. This is within the
design (cancel-before-CAS keeps the source; close never removes a successor), so it is a
visible-state wart, not a durability defect. Suggested: on `finish`, when the request was
cancelled but `progress.ack()` is present and matches the live buffer's slot/generation, still
latch `recovery_ack`/`swapped_version` so the buffer reflects the durable copy it owns.

**M-B — v2 owner directories and orphaned successor records accumulate with no in-app
reclamation, and a failed-import-then-Save-As source is re-offered every launch.**
Symbols: `recovery_store::allocate` (never reuses or removes owner dirs — deliberate);
`recovery_discovery::read_v2` tombstone branch (returns `Ok(None)` only when the lock is free);
`cleanup::cleanup_saved_with_policy` (retires only the buffer's own slot, never a source token).
Trigger (probe P3, confirmed): an import whose handoff fails pre-sync retains its v2 source; a
later successful Save As of the recovered document does not retire that source (the buffer's own
slot never produced a checkpoint), and the source is re-offered at the next launch's bootstrap
scan. `Clean Recovery Files` excludes all of `recovery-v2`, so there is no in-app way to remove
it. This is the design-deferred stable-lock/tombstone reclamation gap (round-1 M-2), widened by
the retained-source case. No data loss; conservative over-retention. Suggested: a bounded
backlog item to reclaim lock-only owner dirs and orphaned successors older than N days under the
exclusive lock, as the triage already contemplates.

**M-C — a contextual auto-offer is dropped, not deferred, when its origin buffer is no longer
active.** Symbols: `recovery_flow::discovery::offer` (the `origin` guard `continue`s, discarding
the offer). Trigger (probe P7, confirmed): open a file whose associated recovery exists, switch
buffers before the scan lands; that file's candidate is never offered automatically again this
session, even after switching back. Manual `Review Recovery Files` still lists it, which the
design explicitly relies on ("Closed original Buffer does not hide candidates; offer through the
manual review command"). Design-sanctioned; noted only because "switch away and back" is an easy
way to miss the one automatic prompt.

**M-D — manual review shows an uninformative busy row for a live, saved buffer whose checkpoint
was retired.** Symbols: `recovery_discovery::read_v2` (takes the owner lock before reading the
record, so a live owner can never reach the tombstone `Ok(None)` branch); `scan` Associated/All
handling of busy rows. Trigger (probe P1, confirmed): a saved-and-clean open buffer holds its
owner lease; its record is already retired; `Review Recovery Files` lists one row
`recovery IO: recovery record is active` with no filename, time, or preview. The design does say
active owners appear unavailable in manual review, so this is conformant, but the empty row tells
the user nothing. Suggested (optional): suppress busy rows that carry no metadata, or label them
as "this session's active document".

### Round-1 Minor items M-3 / M-4 / M-5

The fixes report claims these were addressed; I confirmed by reading the code:
`canonicalize_with_missing_suffix` now has one home in `fsx` (reused by flow and discovery),
`validate_owner` one home in `recovery_store::codec`; scan reads pass borrowed bodies
(`read_v2`/`read_legacy` take a `consume` closure; `swap::parse_borrowed` avoids the owned copy);
the touched flow/picker modules are hand-wrapped. No action.

## What I verified as correct (no finding)

- **Ownership (R1/D4):** exclusive `create_dir_excl` with a 128-attempt bound, lock created
  before any record is published, stable lock inode never unlinked, slot identity by
  `Arc::ptr_eq`, jobs clone the slot so leases outlive buffer drop. Subprocess tests confirm
  named/unnamed/same-path owners never share a directory and that a dead process releases its
  lock while a queued closure retains it (`recovery_process_*`).
- **Strict acknowledgement:** `checkpoint_with_names` consumes the attempted generation before
  IO, writes temp 0600 → flush → `sync_all` → rename → strict owner sync → (until first Ack)
  every ancestor to `/`; Ack only after the full chain; failure keeps `acknowledged` unchanged so
  the whole obligation is re-owed on retry, including across a poisoned mutex and a fresh slot
  (`recovery_ownership_fresh_slot_retries_every_failed_ancestor_barrier`,
  `..._partial_mkdir_retries_keep_full_sync_obligation`). Replayed/unreserved generations refused.
- **Source retirement (R5/D5):** the handoff job writes the successor with the pre-callback body,
  `record_ack`, then `authorize` (durable flag + CAS Pending→Retiring) and only then unlinks the
  source and strictly syncs its parent, all in one FIFO worker operation holding the source
  lease; legacy and retry paths never unlink. Process-crash boundaries preserve source-or-successor
  on restart (`recovery_process_crash_boundaries_preserve_source_or_successor_on_restart`).
- **Save receipt:** `cleanup_saved_with_policy` under the owner guard reopens the committed target
  once, compares full bytes and same-handle fingerprint, syncs the handle then the parent, and
  consumes a non-`Clone` receipt; every IO fault retains the record and never demotes a successful
  save; `panicx::catch` isolates cleanup panics
  (`recovery_save_cleanup_panic_does_not_turn_successful_write_into_failed_save`,
  `recovery_save_and_close_preserves_cleanup_warnings`). The SaveAs association widening keeps
  owner/generation/version/body/fingerprint checks intact.
- **Late-completion safety:** a save merge and a checkpoint merge both guard editing-instance
  identity (`same_instance`) and buffer/version, so a completion cannot mark a replacement buffer
  saved or protected (`recovery_save_late_completion_cannot_mark_replacement_instance_saved`,
  `recovery_ownership_replacement_same_id_cannot_acknowledge_old_slot`).
- **Legacy preservation:** `.swp` and `recovered-*.md` are read by header, never renamed or
  unlinked in scan/prepare/handoff; symlinks/FIFOs are rejected without blocking
  (`O_NOFOLLOW|O_NONBLOCK`, metadata validated on the opened handle).
- **Cancellation/quit:** `has_pending_work` blocks only concrete association/handoff work;
  `after_job` runs before quit re-drive in both apply wrappers; `after_callbacks` runs before the
  `!editor.quit` early return; the 5 s timeout is scoped to an active foreground quit, detaches
  the blocker while preserving a late successful Ack, and cannot reinstate a cancelled quit
  (`recovery_quit_*`).
- **Selector/command surface:** `review_recovery_files` registered in the File category through
  the registry; a single `Recovery` `OverlayId` row in `OVERLAYS`/`ALL`/`RENDER_ORDER` with one
  geometry for paint and hit-test; zero-selection keeps the list open; Esc/click-away/registry
  replacement share `close`. The obsolete `swap_recovery` prompt and `Recover`/`DiscardSwap`/
  `OpenOriginal` actions and `load_recovered` are removed and their tests migrated.
- **Opening flows (R4/D6):** `recovery_flow::opened` is called after successful install in both
  `workspace::open_as_new_buffer` and `session_restore::open_into_current`; bootstrap runs after
  the executor/wake relay exist and before the first blocking receive; discovery is worker-only
  and edge-triggered (no idle scans; `recovery_open_bootstrap_real_worker_wakes_without_keyboard_input`).
- **Resource/IO boundaries:** no filesystem IO in reduce/render/timers; scans read one capped
  record at a time; previews bounded to 240 scalars; `MAX_RECORD_BYTES` consistent with the codec
  caps; the SSD-wear guardrail tests still bound checkpoint writes per edit-version.
- **Regression vs HEAD:** the `SwapWrite` → `Recovery` job-kind migration is complete (no live
  old-format writer); `dispatch_swap_write` is now thin delegation into `recovery_flow`;
  `open_swap_paths` protects open documents, acks, and pending sources through resolved aliases
  and fails closed on an unresolvable path; the panic-dump `recovery.rs` path is untouched.

## Probes

Compiled against the real branch through a temporary `#[path]` `mod` line in `lib.rs`, then
removed; `lib.rs` re-hashed against the manifest afterward (0 mismatches). Source and outputs are
in `review-probes/` (`probes.rs`, `verify_hashes.py`). All five probes passed (they assert the
observed behavior):

- P1 — live saved owner appears as a busy, metadata-less unavailable row in manual review (M-D);
  drops to a hidden tombstone once the buffer/lease is dropped.
- P2 — Esc during Opening after install: source retained, successor written but Ack not latched,
  no protection indicator, no scheduled re-checkpoint (M-A).
- P3 — failed import then Save As: v2 source survives and is re-offered next launch (M-B).
- P6 — same-process second `owner.lock` acquisition returns `WouldBlock` (flock semantics); this
  is what makes live records read as busy, and is the basis for the platform caveat below.
- P7 — a contextual auto-offer is dropped when its origin buffer is not active (M-C).

## Limitations

- Linux only. Windows durability (`FILE_FLAG_BACKUP_SEMANTICS` directory sync, reparse-point
  rejection) is compiled-for but unvalidated here; `validation.json` records the same.
- Process-death probes and the subprocess tests exercise ordering and lock release; none simulate
  power loss.
- `ThreadExecutor::drop` still joins its worker; the 5 s timeout bounds foreground waiting only,
  not OS IO or process exit.
- Lock busy-detection relies on `File::try_lock` (flock-style, confirmed in-process by P6); on
  lock-emulating mounts (some NFS) the busy/tombstone classification may differ.
- Lua event hooks are observer-only; the recovered-Open contract was exercised through a
  registered command that edits/saves (`recovery_plugin_open_observes_queued_handoff_before_command_edit_and_save`).
- I could not reproduce the round-1 intermittent test failure across repeated lib and workspace
  runs, but repeated green runs do not prove a rare race is impossible.
- Probes compiled against private APIs via a temporary `lib.rs` line; the tree was restored and
  hash-verified. The mandated smoke run added one line to the gitignored `scripts/smoke/.history`.
