# Recovery Save As contract check

**Superseded for Save As on 2026-09-15 by explicit user approval.** After a strict
same-handle receipt proves the saved bytes durable at the new destination, cleanup may
retire this editing instance's owned checkpoint with association None or the captured
pre-rekey path. All owner/generation/version/body and strict file/parent-sync gates remain.
Ordinary Save retains the original association rule. The historical analysis below records
why the previous implementation retained the checkpoint; it no longer specifies the final
Save As behavior. Expected rule-based retention is now Info, while IO/uncertainty remains
Warning.


Date: 2026-09-14. Read-only check of design revision 5 section C, plan revision 2
Task 4, and current `save::do_save_to`. No cargo commands or production edits.

**Conclusion:** clean Save As A → B can deliberately leave an owned checkpoint
associated with A. This is a conservative retained copy, not a violation of the
explicit cleanup rules or a loss of recovery data. The narrower technical contract
does not promise every retained on-disk checkpoint is immediately re-associated.

The contract requires checkpoint association to equal the committed destination
before cleanup, and emits an association checkpoint only for dirty content or an
outstanding handoff. It expressly prohibits resurrecting an older clean snapshot.
Therefore an existing A checkpoint cannot be deleted using a B save receipt and
must remain when the accepted Save As leaves the document clean with no handoff.
It remains discoverable manually, although opening B contextually may not offer it.
Further normal saves to B can continue retaining it until a new checkpoint updates
its association. This duplicate/stale-context tradeoff should be documented candidly.

The current legacy implementation deletes both new and dispatch-time prior swap
paths after successful Save As. Task 4 must remove those path-derived deletes;
they are not precedent for giving a B receipt deletion authority over an A record.
The existing merge-time path update and chained session migration remain relevant:
association updates must follow accepted merge order, not stale dispatch-time paths.

## Minimal mechanism within the approved contract

1. Capture the exact slot and eligible snapshot/generation in the save job.
2. Under that ownership, refuse cleanup if the stored association differs from the
   committed chosen association; preserve the record byte-for-byte. Distinguish this
   retained outcome from successful cleanup and surface the recovery-retained warning
   alongside the otherwise successful save.
3. On accepted Save As merge, retain the slot and update foreground association to B.
   Queue a current-snapshot association checkpoint only if dirty or handoff-pending.
   A clean/no-handoff merge does not create a checkpoint just to remove this duplicate.
4. Keep original provenance distinct from current association. Never give the save
   receipt authority over a predecessor source, foreign slot, or legacy file.

Rewriting clean checkpoint metadata before cleanup, weakening association equality,
or scheduling clean association work solely to eliminate the duplicate would alter
explicit reviewed rules. None is needed to implement the present safety guarantee;
if desired later, amend and independently review that contract first.

## Required Task 4 regressions

- Existing A checkpoint + successful clean Save As B: same slot, foreground path B,
  saved state successful, old record unchanged, visible retained-copy result, no
  clean association checkpoint. Cover both Saved and Unchanged outcomes.
- A checkpoint + Save As A: cleanup eligible only after complete same-handle durable
  destination proof; failure at any receipt stage preserves the checkpoint.
- Edits during Save As B: association follow-up captures current dirty content and
  B, never the earlier saved snapshot. Failed follow-up preserves recoverable data.
- Two queued Save As operations A → B → C: accepted merge order determines current
  association; a late ordinary save to A cannot clean C's checkpoint or acknowledge
  C's saved state. Include divergent and newer checkpoint generations.
- Save As with outstanding handoff: ordinary cleanup cannot retire the predecessor;
  required follow-up follows the separate handoff authority and ordering rules.
- Foreign same-path records remain byte-for-byte intact throughout.

This check does not review unfinished discovery work or certify Task 4 implementation.
