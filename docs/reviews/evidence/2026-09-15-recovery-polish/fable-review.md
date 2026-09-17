# Fable independent re-review — recovery polish (M-A cancelled-handoff Ack, M-D busy-row clarity)

Date: 2026-09-15. Reviewer: Claude Fable (independent final gate, polish round).
Snapshot: uncommitted working tree on `fix/recovery-safety`, base `main` `9e409c0`.
Source identity: all 45 files in `docs/reviews/evidence/2026-09-15-recovery-polish/source-sha256.json`
re-hashed in this checkout — 0 mismatches (`review-probes/verify_hashes.py`). Exactly four files differ
from the prior round-2 GO manifest (`previous-final-review/source-sha256.json`): `recovery_flow/import.rs`,
`recovery_flow/import/tests.rs`, `recovery_picker.rs`, `recovery_picker/tests.rs` — matching
`polish-changed-files.json` and `polish-source.patch`. The live changed-file set (tracked diff + untracked
`wordcartel/src`) equals the manifest set exactly. `validation.json`'s manifest hash and all five recorded
gate-log hashes match the files in the evidence directory.

Authority: the scoped design/plan (`2026-09-15-recovery-polish-design-plan.md`), its GO review, the ledger,
the REVIEW_BRIEF constraints, and my round-2 report. I did not infer correctness from the earlier GO or the
green logs; every claim below was checked against the live source and, where behavioral, by compiled probes.

## Verdict: GO

Both refinements do what the plan says, nothing more. The cancelled-completion latch cannot retire a
source, install a cancelled preparation, protect newer edits, touch a replacement slot, or change a later
quit. The busy-row wording invents no ownership or record metadata, does no IO, and busy rows stay
unselectable and auto-offer-filtered. All merge gates pass on this exact snapshot.

Findings: Critical 0, Important 0, Minor 0. Three non-blocking observations are recorded below.

## Gate results (run in this checkout, on the manifest-verified tree)

| Gate (CLAUDE.md) | Result |
| --- | --- |
| `cargo clippy --offline --workspace --all-targets` | PASS, clean (`Finished dev profile`, no warnings) |
| `cargo test --offline --workspace` | green: lib 2183 passed / 2 ignored; backlog 7; edit_seam 2; fs_chokepoint 10; module_budgets 5; sentence_differential 4; core 308 + oracle 42 + 4 integration; nlp 48; remaining suites 15/13/2 — 2,643 passed, 7 ignored, 0 failed (matches `validation.json`) |
| `cargo build --offline --workspace` | warning-free |
| `cargo test --offline --workspace --no-run` | 0 warnings |
| `git diff --check` | clean |
| `scripts/smoke/run.sh` | `smoke: 9/9 PASS` (advisory, quoted verbatim) |
| manifest re-hash (before and after probe wiring) | 45/45 match both times |

## M-A — Ack after opening-batch cancellation (`import::finish`)

The only production change is that the Ack latch (`recovery_ack`, `swapped_version = request.version`,
`last_swap_at = request.started`, `recovery_protection_failure = None`) moved outside the
`!request.cancelled` guard; the protection-failure branch is now `else if !request.cancelled`. Verified
against the live source and cross-module consumers:

- **Slot identity still gates everything.** The latch is inside `by_id_mut(bid).filter(same_instance(slot))`;
  the exact removed request carries the captured `path`/`version`; the Ack must match `document.path` and
  `recovery_generation`. A replacement Buffer with the same id, a closed Buffer, a Save-As'd path, or a
  changed generation receives nothing (regression matrix + probe P5 through the real `close_buffer_now`).
- **No retirement authority.** The worker still does `record_ack` → `authorize()` (CAS 0→2) → unlink only
  when the CAS wins; `cancel_batch` performs `progress.cancel()` (CAS 0→1) before the foreground flag.
  Nothing on the foreground reads `predecessor` for deletion (grep: it is codec metadata and a test
  assertion only); Save cleanup (`cleanup_saved_with_policy`) is keyed on the slot's own owner record, never
  on `recovery_ack`. Probe P2: after a cancelled latch, Save As retires only the buffer's own successor via
  the strict receipt and the retained v2 source survives with an Info status.
- **Cancelled preparations still die.** `drive` still removes any `ready` entry whose request is cancelled;
  `cancel_batch` still clears the queue. Unchanged and re-read.
- **Newer edits stay unprotected.** `swapped_version` records the captured version; `document.version` is
  strictly monotonic (`+= 1` on apply/undo/redo, `editor.rs`), so `protected()`/`swap::pending` cannot
  alias a later edit. Probe P3: an edit made while the cancelled handoff was in flight leaves
  `swap::pending == true`, the cadence arms, `dispatch_checkpoint` is `Accepted`, and a new generation
  covers the edited text — the source is still not retired.
- **Later quit unaffected.** The early `if request.cancelled || request.detached { return; }` still
  precedes `row_result`, status, and `quit::cancel`; `has_pending_work` only sees live requests. Probe P4:
  `wait_for_saves` is false and `quit::after_callbacks` yields byte-identical outcomes (review prompt for the
  dirty recovered buffer) after cancelled vs uncancelled handoffs. Probe P1: the entire post-completion
  buffer state is identical between the two paths except that the cancelled path keeps the source.
- **Error without Ack, cancelled:** no failure recorded, no status — same as before the polish (the whole
  block was skipped then). Error with a durable Progress Ack (panic after `record_ack`): the Ack is latched
  in both cancelled and uncancelled paths; `protected()` is never used to synthesize an Ack.
- **Retry bookkeeping** (`succeeded`/`failed`, `arm_retries`) and worker IO are untouched.

The regression `recovery_polish_cancelled_handoff_latches_only_matching_durable_snapshot` covers none /
edit / path / generation / replacement / closed / failure / panic_after_ack with source retention, quit
flag, status-history and remaining-selection assertions; I read it in full and it asserts the right things.

## M-D — Metadata-less busy rows (`recovery_picker::paint_row` / `paint_details`)

- **Where `busy` comes from.** Only `recovery_discovery::unavailable()` sets it, and only for
  `RecoveryError::Io` with `ErrorKind::WouldBlock`, which `fsx::try_recovery_lock` produces solely for
  `TryLockError::WouldBlock` ("recovery record is active"). `open_regular_nofollow` cannot produce it
  (O_NONBLOCK is used to avoid FIFO blocking; non-regular files are rejected by metadata). So "An editor is
  using these recovery files" is the accurate, only real cause; it names no session, owner, cleanliness, or
  record state. Busy rows always have `token: None`, `unavailable: Some(..)`, empty preview, `Unknown`
  time — the renderer adds no metadata (probe P6 renders "time unknown" and an empty preview).
- **Unselectable and unofferable.** `toggle`/`accept` still require `unavailable.is_none() && token.is_some()`;
  `discovery::offer` still filters `row.busy`. Probe P6 drives the real manual-review scan against a live
  in-process owner lease, presses Space + Enter through `recovery_picker::intercept` (no selection, no
  dispatch, "Select a recovery file"), and confirms `bootstrap` never auto-offers the row.
- **No IO.** Rendering only reads `Candidate` fields; the file header's "no filesystem IO" contract holds.
- **Reports cannot be hidden.** `row_result` matches rows by token; busy rows have none and
  `PrepareOutcome::Changed` rows come from `read_v2` with `busy: false`, so the "busy: in use" override
  never masks a prepare report.
- **Path stays inspectable.** The details line still shows the source path (probe P6 shows the full
  `…/recovery-v2/<owner>/checkpoint.wcr`); the list row no longer leads with `checkpoint.wcr` or the raw
  "recovery record is active" text. Geometry, path scrolling, tiny-terminal and mouse tests are unchanged
  and green.

## Observations (non-blocking; no fix required for this addendum)

- **O-1 — the "known association/provenance" busy-name branch is unreachable from production scans.**
  `unavailable()` never populates `association`/`provenance`, so `paint_row`'s `row.busy && association.is_none()
  && provenance.is_none()` guard is always true for real busy rows; only the hand-built row in
  `recovery_picker_long_path_cannot_hide_row_time_or_unavailability` exercises the other arm. Harmless,
  cheap, and matches the plan's "preserve known filenames when available" wording; noting so nobody reads
  that test as evidence of a real scanner state.
- **O-2 — after a cancelled latch the source is re-offered next launch beside its successor.** The source is
  retained by design (cancel won the CAS) and the successor carries only `predecessor` metadata; the scan
  does not dedupe by predecessor, and re-selecting the source in-session now focuses the protected buffer
  (`focus_protected`) rather than reinstalling, so the source is never retired this session. This is the
  user-deferred M-B reclamation class (round-2 M-B/P3), unchanged by this polish and not a regression: before
  the polish a re-selection went through the retry path, which never retires either.
- **O-3 — the CAS race variant is now better, not worse.** If `cancel_batch` lands after the worker's
  `authorize()` succeeded, the source is retired and `request.cancelled` is still set; before the polish that
  successor went untracked, now it is latched. Silent by design (cancelled requests emit no status).

## Probes

Compiled against the real branch via a temporary `#[cfg(test)] #[path = "../../review-probes/probes.rs"]`
line in `wordcartel/src/lib.rs`, then removed; `lib.rs` re-hashed against the manifest (0 mismatches).
Source: `review-probes/probes.rs`; output: `review-probes/probes-output.log` (transcribed — shell
redirection was not permitted in this session); manifest check: `review-probes/verify_hashes.py`.

```
cargo test --offline -p wordcartel --lib review_probes -- --nocapture --test-threads=1
test result: ok. 6 passed; 0 failed
```

- P1 — cancelled vs uncancelled handoff: identical latched state (ack gen 1, swapped 0, last_swap 7,
  no failure, retry Ready, no pending work); only source retention differs.
- P2 — cancelled latch → Save As: own successor retired by receipt, source retained, status Info.
- P3 — edit during cancelled handoff: unprotected, cadence armed, fresh checkpoint (gen 2) covers it.
- P4 — later quit: `wait_for_saves` false; `after_callbacks` outcome identical in both paths.
- P5 — real `close_buffer_now` during handoff: batch cancelled, no Ack on the surviving buffer, no status,
  source + orphan successor both on disk (M-B class).
- P6 — real busy row from a live lease: label/explanation/path as designed, no ownership claims,
  unselectable, not auto-offered.

Two probe-harness corrections were needed and are not findings: P3 initially bypassed `app::reduce`, which
is where `last_edit_at` is armed (app.rs `if version != before`), so the probe models that one reducer
line; P4 initially inverted the `after_callbacks` return polarity (true = keep running).

## Limitations

- Linux only; Windows compilation/runtime durability (directory sync semantics, reparse-point rejection)
  remains unvalidated here, as `validation.json` records.
- Process-death tests and probes exercise ordering and lock release; none simulate power loss.
- The 5 s quit timeout bounds foreground waiting only, not OS IO or worker thread joining.
- Lock busy-detection relies on `File::try_lock` flock semantics; lock-emulating mounts may classify
  busy/tombstone differently, which would also change which rows receive the new wording.
- Probe output in `probes-output.log` is a faithful transcription of the terminal run, not a redirected
  capture. The mandated smoke run appended one line to the gitignored `scripts/smoke/.history`.
- No live-tree edits remain: the only working-tree additions beyond the reviewed snapshot are
  `review-probes/` and this report.
