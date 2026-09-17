# Fable final-review follow-up

Original report and probes are retained in
`evidence/2026-09-14-recovery-implementation/fable-review.md` and `fable-probes/`.
The first snapshot captured final validation in progress; it is not the final gate evidence.

| Finding | Disposition |
| --- | --- |
| C-1 test-module inception | Fix parent module names via explicit test-file paths; retain inner cfg(test) module for filesystem source guard. Revalidate both guards. |
| I-1 background deadline / lost Ack | In progress: scope timeout to foreground quit waits and preserve safe late acknowledgement routing. |
| I-2 clean Save As retention | User decision requested: permit owned-checkpoint retirement after exact durable saved-content verification at new destination. Current rule is approved design, not an implementation deviation. |
| I-3 repeated unavailable offers | In progress: session dismissal must include unavailable rows while manual review remains exhaustive. |
| I-4 failure retry loop | User decision requested: include bounded retry delay here or preserve explicit R3 follow-up scope. |
| I-5 parallel lease-test race | Bounded acquisition helper only at deliberately released-lease assertions; active-owner Busy assertions remain immediate. |
| I-6 gate evidence | Final logs will be captured after all fixes with source manifest and commands. Earlier clippy commands did include --workspace --all-targets; dev-profile stdout does not distinguish targets. The wrapper change happened after that successful clippy run. Initial snapshot copied workspace output while its run was still active. Both limitations are recorded; no final gate is claimed on that snapshot. |
| M-1 cancelled contextual scan intents | Inspect and fix scoped cancellation with production fixer. |
| M-2 tombstone reclamation | Explicit design deferral: stable lock inode must not be removed while leases may refer to it. No new reclamation policy is inferred from this implementation effort. |
| M-3 duplicate helpers | Consolidate shared validation and path resolution. |
| M-4 transient scan body copy | Avoid allocating an owned full body for candidate-only scans. |
| M-5 house style | Hand-wrap revised flow modules; no rustfmt. |
| M-6 expected retention Warning | Coupled to pending Save As decision; distinguish expected retention from IO uncertainty. |

Independent Codex's F1 repeat-selection finding was fixed and re-reviewed GO before
this Fable follow-up. Its pre-fix snapshot remains immutable for audit. The original
Fable report's platform statement is not cross-platform evidence: this effort has
Linux runtime evidence; Windows compilation/runtime durability remains unvalidated.

## User decisions and retry design amendment — 2026-09-15

The user approved both requested changes:
- "Yes—retire it after durable verification (Recommended)" for the owned checkpoint
  after Save As. The explicit receipt policy changes association eligibility only;
  ownership, captured generation/version, exact contents and durable saved-file proof
  remain mandatory.
- "Yes—add a bounded retry delay (Recommended)" for the previously deferred retry loop.

The retry implementation uses a separate 30-second failure floor, applied to automatic
checkpoint scheduling independently of last edit/last successful checkpoint timestamps.
A failure is marked by its result and armed once at the foreground merge clock boundary,
so slow IO does not consume the delay before the failure is reported. More typing does
not bypass the floor. Success clears the floor and ordinary edit-driven idle behavior
continues. Queue rejection and panic take the same path. This addresses immediate
failure retries; other R3 scheduling questions remain outside this narrow amendment.
Protocol-directory ownership/mode validation is not weakened or silently repaired.
