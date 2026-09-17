# Recovery safety brainstorm — R1, R4, R5

Date: 2026-09-11. Baseline: main after merge 9e409c0.
Status: discussion draft. D1 (separate recovered document), D2 (first-save
Save As behavior), D3 (selection list), D4 (independent recovery ownership), and
D5 (verified handoff), and D6 (Review Recovery Files command) are approved;
other recommendations remain proposals. Last decision recorded: 2026-09-14.
No production changes or new commits accompany this draft.

## Grounding

- `swap::swap_path` names unnamed copies by PID and named copies by path hash;
  `delete_with_fs` recomputes that shared name. This permits cross-buffer and
  cross-process replacement/deletion (R1).
- `SwapHeader.id` already carries `DocumentId` as a lineage hint. It is not read by
  discovery today and is not an exclusive ownership token. Do not silently repurpose
  its existing lineage semantics or assume its 64-bit hash is collision-proof.
- Startup assesses recovery; `workspace::open_as_new_buffer` and
  `session_restore::open_into_current` construct buffers without equivalent assessment
  (R4). Open plugin callbacks currently follow construction.
- `PromptAction::Recover` loads a replacement buffer then removes the old orphan;
  it does not wait for a replacement checkpoint (R5).
- `swap::write_atomic_with_fs` uses shared atomic replacement with `dir_fsync: false`.
  `RealFs::sync_dir` also treats failure to open the directory as success. Atomic
  visibility alone is insufficient evidence of power-loss durability.

## Candidate architectures

1. Independent recovery records per editing instance (recommended starting point).
   Stable, collision-checked ownership across Save As; path and document lineage are
   discovery metadata. Each record is independently discoverable. A directory scan
   can rebuild an optional index; losing an index must not hide the only copy.
2. Session directory with a manifest. Groups crashed work naturally, but a manifest
   introduces another consistency problem. Individual records must remain discoverable
   after missing/partial manifest updates.
3. Immutable checkpoint generations with a journal/index. Preserves history and
   enables stronger rollback, at the cost of storage, garbage collection, and a
   substantially larger verification surface. Defer unless history is a requirement.

## Proposed invariants and mechanisms

- A save may retire only an exact recovery record owned by that editing instance,
  covering content known to have been saved. A path match never grants deletion rights.
- Claim new record names with exclusive creation/retry; random IDs alone do not
  constitute a collision guarantee. Keep editing-instance identity distinct from lineage.
- Track checkpoint request/generation in async results. Stale completion cannot
  acknowledge a different owner, newer checkpoint, replacement buffer, or Save As
  association. Test queued writes crossing save, close, and recovery transfer.
- Common open assessment returns all relevant candidates for every supported entry
  path. Decide when plugin Open events fire and how commands behave while recovery
  decisions are pending; a callback must not bypass protection of unreviewed copies.
- Choosing the disk version does not imply discarding recovery. Explicit discard
  identifies a particular candidate; concurrent adoption requires a claim/lock protocol
  or conservative retention. Do not infer exclusive authority from PID liveness.
- Recovery handoff: preserve source -> load content -> dispatch immediate checkpoint
  under new ownership -> confirm durability -> retire the source if authorized.
  On checkpoint or sync failure preserve the source and show the failure.
- Crash midway may leave duplicate recoverable records. Record provenance so they can
  be grouped without treating matching timestamps or hashes as proof of equivalence.
- Save As retains owner identity; association with the new path must survive restart.
  Define what recovery offers if the process dies before association is checkpointed.
- Keep legacy records discoverable and protected. Reading an old record must not cause
  destructive migration. Transfer only through the same verified handoff.
- Recovery imports need immediate checkpoint eligibility independent of another edit.
  Broad scheduling repairs (R2/R6/R9/R3/R11) remain a later effort.
- Reads stay bounded; discovery/IO stays off typing paths. No periodic heartbeat writes
  solely to mark a session alive. Determine portable lock/liveness semantics explicitly.

## Product possibilities to decide

**Approved decision D1 (2026-09-11): open recovered work as a separate document
alongside the disk version.** User response: "Yes, open as separate document."
Recovering does not replace the disk-version buffer. Preserve the original recovery
record until a verified handoff; the new document remains unsaved user work.

**Approved decision D2 (2026-09-11): the recovered document starts without a save
destination.** User response: "yes, use that behavior."
Its first Save uses Save As, with the original location and a distinct recovered
filename suggested where available. The original path is provenance, not an automatic
write target. Choosing the original file explicitly still requires the applicable
conflict/overwrite handling. Filename policy and how to handle an original that is
missing remain to be specified. A suggested name must not silently overwrite an
existing file; ordinary destination checks and overwrite confirmation still apply.

Multiple candidates should be visible with a content preview, original path when
known, and useful timestamps whose clock semantics have been checked. Do not select
one silently by maximum timestamp. A future recovery shelf could make preserved
candidates reachable without repeatedly blocking startup.

**Approved decision D3 (2026-09-11): when several recovery candidates exist,
present a selection list and let the user choose which to open.** User response:
"yes, use that selection-list behavior". Include the original filename when known,
checkpoint time, and a short content preview to distinguish candidates.
Opening candidates creates separate recovered documents under D1/D2. Unselected
candidates remain on disk; dismissing the list is not a discard decision. Do not
silently choose the newest candidate or automatically open every candidate.
This approves the selection list; a larger recovery browser is not yet designed.
Preserved candidates can be revisited through the command approved in D6.

**Approved decision D4 (2026-09-11): use independent recovery records per editing
instance (candidate architecture 1).** User response: "yes, use this model". Two buffers/processes editing the same named file own
separate records; unnamed documents also have separate records. Ownership remains
stable across Save As. The path and lineage identify associations for discovery,
not authority to overwrite or delete another instance's record. Record allocation
must be collision-checked; exact generation and handoff rules need design review.

**Approved decision D5 (2026-09-11): recovery uses a verified handoff.**
User response: "Yes, use this handoff behavior." Keep the original
recovery record while opening the separate recovered document. Immediately schedule
a checkpoint under the new editing instance's ownership, independently of another
edit. Retire the original only after that replacement is confirmed durably stored
and the retirement is authorized for that exact source record. Concurrent adoption
and old-format records require conservative handling; successful local copying alone
does not grant authority to delete another instance's live record.

If checkpointing or durability confirmation fails, preserve the original, keep the
recovered document open, and show a clear warning. Retry behavior must avoid busy
loops. A crash during handoff may leave both records; discovery must keep them
available. This behavioral guarantee is approved; filesystem sync semantics,
request/generation checks, and crash-boundary tests remain design work.

**Approved decision D6 (2026-09-14): add a "Review Recovery Files…" command that
reopens the selection list during a session.** User response: "yes, let's add a
\"review recovery files\"". Dismissal leaves candidates intact and accessible
through this command. Retain contextual discovery at startup and when opening a
related document; do not repeatedly interrupt merely because a list was dismissed.
The command must use the registry, appear in the command palette and an appropriate
menu, and follow the command-surface contract. Existing Clean Recovery Files remains
a distinct action; opening the review list never implicitly authorizes deletion.

All six product decisions are recorded. Consolidated requirements:
[recovery safety design draft](../superpowers/specs/2026-09-14-recovery-safety-design.md).

After this product decision, consolidate the approved behavior and proposed technical
mechanisms into a design for independent review. Do not seek separate user approval
for routine implementation details; bring back only substantive product or guarantee
changes uncovered by grounding or review. No production implementation is approved
by this brainstorming document alone.


## Verification requirements for a later plan

Reproduce existing R1/R4/R5 cases on the baseline and convert them into regressions.
Cover multiple unnamed buffers; duplicate named paths within/across processes;
Save As and queued jobs; startup and in-session opens; multiple candidates; failed
replacement checkpoint; crash boundaries before/after checkpoint acknowledgement
and source cleanup; legacy records; corrupt/oversized input and failed discovery.
Use fault-injected IO and process-crash tests with explicit limits on what they
prove about physical power loss. Independently review design and plan before coding.

## Design/planning outcome — 2026-09-14

The technical design (revision 5) and implementation plan (revision 2) both passed
independent Codex review with zero findings. D1–D6 are unchanged. See the
[progress and review ledger](2026-09-14-recovery-design-progress.md) for corrections,
the scoped plan-format decision, evidence, and links. Production implementation has
not started.
