# Recovery safety — independent Codex spec review, round 1

Date: 2026-09-14. Reviewed technical proposal revision 1 against source at
`9e409c0` and `2026-09-14-recovery-grounding.md`. Static review only: no cargo,
source edits, or delegation. Symbol names below are source navigation anchors.

**Design compliance: NOT READY. Quality: NOT READY. Gate: NO-GO.**

The six approved product decisions are represented faithfully. Independent owner
allocation, stable lock inodes, copy-only legacy import, separate recovered
buffers, request identity, and a shared post-callback hook are sound directions.
However, the handoff proof and several lifecycle decisions are not yet sufficiently
specified for implementation. No Critical finding; four Important and two Minor.

## Important I1 — Ordinary save success is not the strict durability receipt required for handoff

Proposal B permits an ordinary save to establish the durable successor; C lets
successful saves clean owned checkpoints; E permits source retirement on a save
receipt. But `file::save_atomic_with_fs` returns `Unchanged` immediately after a
byte comparison, with no file or directory sync. Its `Saved` path uses
`fsx::atomic_replace` with existing `Fs::sync_dir`; `RealFs::sync_dir` swallows
directory-open errors. `save::do_save_to` treats Saved and Unchanged alike.

Consequently, interpreting current Save success as the new strict receipt can
delete the source after a skipped sync or swallowed directory error. Merely naming
a result "durable" does not supply a new save implementation contract. This
contradicts the new checkpoint acknowledgement rule and failure matrix.

**Correction:** define a distinct recovery-authorizing save receipt and its exact
IO sequence (including Unchanged), or forbid ordinary save receipts from retiring
handoff sources and retain them until a strictly acknowledged checkpoint exists.
Also specify whether owned-checkpoint cleanup requires the same stronger receipt.
Keep user save success separate from protection/cleanup success. Fault coverage
must include an identical target whose sync fails and directory-open failure after
an otherwise successful user save.

## Important I2 — A historical successor receipt does not prove that successor still exists

Proposal A stores one mutable `checkpoint.wcr` per owner. E says the initial body
stays protected until its own successor is acknowledged, and says a newer divergent
checkpoint cannot justify source deletion without tracking the initial receipt.
Tracking that receipt alone is insufficient with this storage model.

Possible ordering: source body A is imported; checkpoint A commits; checkpoint B
(new edits) runs before the foreground handles A's receipt; foreground accepts A's
receipt and queues source retirement. The owner now contains B, not A. Even when B
is queued after receipt acceptance, retirement is a later job and can run after
B. `jobs::ThreadExecutor::new` runs subsequent FIFO work without waiting for a
foreground acknowledgement; `jobs_apply::apply_job_outcome` cannot make those two
phases atomic. Owner locks prevent another process, not later jobs sharing the
same owner handle.

This leaves the stated exact-initial-body guarantee dependent on unspecified
generation pinning. The same dependency must be settled before own-save cleanup
can remove a successor referenced by a still-pending retirement request.

**Correction:** explicitly pin the required successor generation against overwrite
and cleanup through retirement, or revalidate the still-current exact successor
inside the retirement transaction and retain the source if superseded. Specify
what happens to queued edits and saves while pinned and how a superseded handoff
becomes terminal without repeated work. Add deferred-executor coverage where two
checkpoint jobs finish before the first result merges, and where Save As/cleanup
lands between receipt acceptance and retirement.

## Important I3 — Failure retention, multi-selection, and quit lack a coherent terminal-state contract

A prohibits holding two pre-existing source locks at once. E processes selected
imports sequentially and retains each source lease until successor success or
cancellation. B leaves a failed imported document open, with no automatic retry.
For selections A and B, if A's checkpoint fails, the text does not say whether B
is indefinitely blocked, A's lease is released, or the lock constraint is broken.
The state diagram labels SourceRetained without defining ownership or subsequent
queue advancement. Repeated recovery is also supposed to focus the existing
Buffer and retry, including after a source-lock release.

C says quit waits for required handoffs/association work. There is no distinction
between active jobs and an unprotected imported document waiting for an optional
edit/retry. Current `jobs_apply::drive_quit_drain` waits only for concrete save
requests, while `quit::review_discard` records discard versions without closing
the Buffer. Adding a generic "handoff exists" wait would therefore strand quit
after protection failure or a deliberate discard unless cancellation is explicit.

**Correction:** define a terminal-state table for prepare failure, checkpoint
failure/panic, retirement failure, cancel, close/reload, and quit discard. Each row
must state lease ownership, request-table removal, next-selection advancement,
retry identity/revalidation, and whether quit waits or cancels. A safe option is
to release failed sources without retirement, continue the batch, and reacquire
and revalidate on explicit retry; no destruction after an unvalidated reacquire.
Specify timeout behavior for genuinely pending jobs separately from failed
protection, plus the behavior of imports prepared before quit but merged during it.

## Important I4 — Selection revalidation requires bytes that discovery explicitly does not retain

D retains metadata and bounded previews, not full candidate bodies. E nevertheless
requires exact bytes against a selected snapshot token and rejects a changed
candidate. No representation or lifetime for that exact-byte token is defined.
A hash is explicitly insufficient for authorization, and storing every body
would contradict the bounded discovery strategy.

For v2, a validated owner/generation identity can work under the immutability and
exclusive-lock protocol. Legacy `.swp` and plain dump candidates have no equivalent
immutable generation; old writers can replace them between discovery and import.
This is material to selecting the text the user actually previewed, even though
copy-only import avoids destructive legacy cleanup.

**Correction:** specify the concrete candidate token separately for v2 and legacy.
For example, use immutable v2 identity/version, and a copy-only legacy reread with
a refreshed preview/confirmation if observational metadata/content discriminator
changes; make clear that this is change detection, not deletion authority. If exact
legacy snapshot identity is mandatory, choose an explicit retained snapshot or
file-handle strategy and account for its resources. Define same-size replacement
and unavailable-old-source cases. Do not leave implementers to choose between
violating the memory contract and inventing a hash-based safety proof.

## Minor M1 — Empty selection and dismissal-set transitions are unspecified

E describes Enter opening selected rows but does not state zero-selection behavior.
D records dismissed exact generations but does not state whether confirming a
subset counts the unselected displayed candidates as dismissed. Without the latter,
the same preserved candidate can interrupt the next contextual open immediately
after a successful subset selection, contrary to D6's no-repeat-interruption goal.

**Correction:** define zero-selection Enter (remain open with feedback is simple),
and exactly which displayed candidate generations enter the automatic-offer
suppression set after Esc, click-away, registry replacement, or subset confirmation.
Manual review must ignore that set as already specified.

## Minor M2 — The proposed document has conflicting gate status

The header says the technical design is incomplete, while the appended revision
says it resolves that work. The "Technical design work still required" checklist
also remains written as outstanding rather than mapped to proposal sections.

**Correction:** label the artifact consistently as a complete technical proposal
pending review/revision, and map the earlier checklist to sections or replace it
with an actual remaining-work list. Preserve the distinction between product
approval and technical-gate approval.

## Re-review conditions

Resolve I1–I4 with executable lifecycle/IO contracts, resolve M1–M2, then re-review
the revised specification against the same source. These are engineering mechanism
choices within D1–D6; this review does not request a change to approved product
behavior. Do not advance to the implementation plan while this gate is NO-GO.
