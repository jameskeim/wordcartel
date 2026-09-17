# Recovery safety implementation plan — R1, R4, R5

Date: 2026-09-14. Baseline: `main` at `9e409c0`.
Status: revision 2, awaiting independent re-review. P1/P2/P4 mechanisms revised;
P3 format discrepancy is governed by the linked scoped format decision. Do not execute
until the independent technical plan gate clears.
Authority: [design revision 5](../specs/2026-09-14-recovery-safety-design.md),
[clean independent spec gate](../../reviews/2026-09-14-recovery-spec-review-round5.md).
Grounding: [lifecycle census](../../reviews/2026-09-14-recovery-grounding.md) and
[IO feasibility](../../reviews/2026-09-14-recovery-io-grounding.md).

This document plans future implementation. No production implementation is authorized
by approval of this document alone. Complete the independent plan gate before execution.
Use a fresh effort branch when execution is authorized. No automatic commit, merge,
push, or formatting commands. Existing unrelated scratchpad files are not task inputs.

## Execution contract

The [scoped plan-format decision](../../reviews/2026-09-14-recovery-plan-format-decision.md)
governs this effort's algorithm/interface artifact format. It is the coordinating
agent's explicit workflow exception, not a user-authored waiver or a claim of literal
CLAUDE.md complete-code compliance. Original P3 remains in the independent ledger;
no implementation safety, per-task code review, test, or final gate is waived.

Execute tasks in dependency order below. Each task starts with a meaningful failing
regression, records its red output, implements the specified contract, then records
green output and gets independent spec-compliance AND code-quality verdicts. A test
which fails only because its proposed API does not exist is not the final red evidence:
where possible scaffold the API conservatively and demonstrate the behavioral failure.
Do not weaken source/chokepoint/module-budget tests to accommodate feature code.

This is a source-grounded algorithm and interface plan, not a purported compiling
patch. Complete compilable reference code is included for the cancellation primitive
and test executor. Other proposed signatures below are interface specifications; their
bodies are prescribed by ordered algorithms and fault assertions, not omitted Rust
bodies disguised with TODO or ellipses. The implementer writes the concrete code against
the named source seams. Every safety decision remains governed by design revision 5.
Keep modules cohesive and functions within repository budgets; do not grow app dispatch.

Every task command below runs twice (red, then green) with log redirection into
`docs/reviews/evidence/2026-09-14-recovery-implementation/`. These are future paths.
Use isolated XDG state/config/cache and a test-injected recovery root; never inspect or
clean a user's real recovery files in tests. Run socket-dependent workspace tests and
private-tmux smoke with the normal sandbox escalation when required.

## Shared proposed types and responsibilities

Existing `Ctx` already transports `Arc<dyn fsx::Fs + Send + Sync>`; keep that single
injection seam. Extend `Fs`, rather than adding a production fallback to RealFs.
All raw filesystem/platform implementation stays in `fsx.rs` (the existing scanner's
sole whole-module exemption). Store/flow/picker modules use injected methods only.
Public Fs signatures require public documented return traits; do not expose a private
type through this public trait. New backend methods default to typed Unsupported for
old unrelated wrappers; never default to successful no-op durability.

Proposed public IO interfaces (each method returns `std::io::Result`):

| Receiver | Method and result | Contract |
| --- | --- | --- |
| `Fs` | `create_dir_excl(&self, path: &Path, mode: u32) -> Result<()>` | Nonrecursive exclusive directory; AlreadyExists means collision, no adoption |
| `Fs` | `validate_private_dir(&self, path: &Path) -> Result<()>` | No-follow real directory; Unix owner/private mode validation under trusted state root |
| `Fs` | `try_recovery_lock(&self, path: &Path, create: bool) -> Result<Box<dyn RecoveryLease>>` | New lock file uses exclusive creation; existing source requires existing file; Busy distinguished from unsupported/error |
| `Fs` | `open_regular_nofollow(&self, path: &Path) -> Result<Box<dyn RecoveryRead>>` | Same opened handle metadata validated regular before read; rejects symlink/reparse/FIFO |
| `Fs` | `sync_dir_strict(&self, path: &Path) -> Result<()>` | Directory open AND sync failures propagate |
| `RecoveryRead: Send` | `read_capped(&mut self, limit: u64) -> Result<Option<Vec<u8>>>` | Reads at most limit+1; None means oversized, not empty |
| `RecoveryRead: Send` | `stat(&self) -> Result<FileStat>` and `sync_all(&self) -> Result<()>` | Both operate on this handle; never reopen by path |
| `RecoveryLease: Send + Sync` | No public methods | Guard owns one locked handle; final Drop only closes; no explicit unlock/reacquire |

Keep owner/receipt constructors private to `recovery_store`. Proposed domain contracts:

- `RecoverySlot`: cloneable Arc of lazy owned state; memory-only construction. Worker
  allocation publishes a lease once. Slot state retains the complete ancestor-sync
  obligation set across allocation/write/rename/sync failure and panic; obligations are
  cleared only after the full strict checkpoint sequence succeeds. Record obligations
  before publishing the lease or attempting checkpoint IO, never only on the stack. Buffer and every queued job clone it. Foreground
  identity comparison uses Arc identity, never its lock or filesystem path. A private
  `Mutex` protects allocation/metadata on the worker; foreground never waits on it.
- `RecoveryOwner`: validated opaque directory component, exclusive allocation decides
  uniqueness. Suggest components using existing minting entropy plus a collision counter;
  neither entropy nor counter alone establishes ownership. Collision retry has a finite
  bound (128 attempts), then visible allocation error; never reuse tombstones.
- `RecoveryGeneration(u64)`: checked increment at capture, independent of edit version;
  start at 1. Consumed generations are not reused even when write/sync fails. Keep
  attempted generation distinct from acknowledged generation. An uncertain rename can
  expose a newer generation; later retry must not mutate that generation in place.
- `RecoveryRequestId(u64)`: editor-local checked counter, never wrap; carried in
  `JobKind::Recovery(id)` through normal and panic outcomes. Public because JobKind is
  public, with crate-only mint and private field, matching SaveRequest's API pattern.
  Export the id from public `jobs` (or a public domain facade in lib.rs); keeping its
  defining module private without a public reexport is not a usable public contract.
- `RecoveryCandidate`: metadata, bounded preview (first 240 Unicode scalar values),
  availability/error and selection token; no retained full body. Preview is display-only.
- `SelectionToken`: V2(owner,generation), or Legacy(raw path,length,mtime,full-body hash).
  Legacy validation is observational, not exact frozen identity or deletion authority.
- `PreparedRecovery`: owned body, provenance, exact selected token, optional source
  lease. It moves worker→result→accepted transaction; never clones a source lock.
- `RecoverySaveReceipt`: private, non-Clone, created/consumed inside a single worker
  operation, binds owner slot, saved version/body, committed association/fingerprint.
  No receipt in Editor, JobResult, or a later queued cleanup operation.
- `RecoveryState` on Editor: request map, checked next id/scan epoch, queued intents,
  deferred offers, selector state, exact dismissed tokens, opened-token→BufferId map.
  Each map entry records operation, BufferId+slot identity when relevant, captured
  generation, start time, exit-blocking flag and shared handoff progress. Closed-buffer
  entries remain until results drain. No global bool may clear another request.
- Buffer owns slot, generation allocator, optional provenance/suggested name and visible
  protection failure. Preserve ordinary cadence fields (`last_swap_at`, `swapped_version`)
  until the scheduling follow-up. Replace boolean-only in-flight ownership with matching
  recovery request identity. Ordinary policy remains in `swap::due/pending`.

### Complete reference cancellation primitive

Place in `recovery_store` (private to the domain; expand visibility only to flow/tests).
The worker writes durable progress before attempting retirement authorization. The
request map retains the Arc even through panic. `finish` is terminal bookkeeping only.

```rust
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const PENDING: u8 = 0;
const CANCELLED: u8 = 1;
const RETIRING: u8 = 2;
const FINISHED: u8 = 3;

#[derive(Default)]
struct HandoffProgress {
    state: AtomicU8,
    successor_durable: AtomicBool,
}

impl HandoffProgress {
    fn cancel(&self) -> bool {
        self.state.compare_exchange(PENDING, CANCELLED,
            Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
    fn authorize_retirement_after_sync(&self) -> bool {
        self.successor_durable.store(true, Ordering::Release);
        self.state.compare_exchange(PENDING, RETIRING,
            Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
    fn is_protected(&self) -> bool {
        self.successor_durable.load(Ordering::Acquire)
    }
    fn finish(&self) { self.state.store(FINISHED, Ordering::Release); }
}
```

Only call authorization after all strict successor syncs. Legacy uses the durable flag
but performs no unlink even if CAS wins. A panic before finish leaves progress readable;
flow removes the exact request and never infers unprotected solely from missing success.

## Task 1 — Injectable strict IO and deterministic harness

Files: `fsx.rs`, `test_support.rs`, `wordcartel/Cargo.toml`, lockfile if needed;
new `recovery_regressions.rs` registered under `#[cfg(test)]` in `lib.rs`.

Add exact target dependencies: Unix `libc = "=0.2.186"` and
`rustix = { version = "=1.1.4", features = ["process"] }`; Windows
`windows-sys = { version = "=0.61.2", features = ["Win32_Storage_FileSystem"] }`.
Safe Unix OpenOptions uses O_NOFOLLOW|O_NONBLOCK, plus O_DIRECTORY for directories;
validate opened metadata before reading/locking. Windows uses OPEN_REPARSE_POINT,
BACKUP_SEMANTICS and rejects FILE_ATTRIBUTE_REPARSE_POINT/nonregular leaves.
Read opened directory ownership using MetadataExt::uid and compare it with safe
`rustix::process::geteuid().as_raw()`. Require effective-UID ownership and no group/other
permission bits for protocol-private dirs; do not use USER/UID environment variables,
libc::geteuid unsafe calls, or infer ownership from mode alone. The configured state
root remains the stated trusted boundary; do not silently impose 0700 on arbitrary
ancestors. Cached rustix 1.1.4 source verifies process/id.rs::geteuid, ugid.rs::Uid::as_raw
and Cargo.toml's process feature; this is feasibility, not a Windows/runtime guarantee.
Inject wrong-owner metadata at the directory-validation helper and assert refusal before
record read/lock/creation. Test the real current-owner path on Unix without requiring chown.
Use File::try_lock exactly once on the opened handle, distinguish WouldBlock. New lock
creation is 0600, never truncate or recreate existing locks. No unsafe code.

Strict provisioning walks missing directories under the trusted configured state root,
creates with 0700 and validates existing protocol directories. Every new slot
conservatively owes strict sync of its FULL resolved directory chain: owner, recovery-v2,
state root, and every ancestor through filesystem root, regardless of path existence.
This first-checkpoint obligation is retained until full acknowledgement; later
checkpoints for that acknowledged slot may require only owner-directory sync. Do not use `swap::state_dir`'s
untracked create_dir_all to claim durable provisioning. Factor a noncreating state-root
path resolver from it; explicit test roots bypass environment mutation. Sync leaf-to-root through the complete recorded chain. A mkdir failure before any
record/lease publication cannot evade this rule: a same-slot, different-slot or
reconstructed-store retry computes/retains the full first-checkpoint chain afresh.
Existing-directory observations do not discharge an obligation; unsupported ancestor
sync fails closed with no Ack or retirement.
On unsupported targets/sync return Unsupported, retain copies and report limitation.

Extend FaultFs with recovery directory-create, lock busy/error, no-follow open/read,
strict-dir-open, strict-dir-sync and same-handle-sync failures. Add an operation journal
with per-occurrence injection (e.g. fail third strict sync), not just a single global
SyncDir flag. A separate test wrapper may compose FaultFs to record barriers/leases.
Update actual implementations: RealFs; FaultFs; three `settings.rs::FailFs` blocks;
`file_browser.rs::CountingFs` (defaults are acceptable for unrelated tests). Do not route
recovery faults around ctx.fs. Tests assert Send+Sync on shared lease type.

Complete reusable test executor (put inside cfg(test) support; completion must use
production wrappers with the caller's SAME injected Fs, not test_fs()):

```rust
use std::cell::RefCell;
use std::collections::VecDeque;
use crate::jobs::{Executor, Job, JobOutcome};

#[derive(Default)]
pub(crate) struct DeferredRecoveryExecutor {
    pending: RefCell<VecDeque<Job>>,
}
impl Executor for DeferredRecoveryExecutor {
    fn try_dispatch(&self, job: Job) -> Result<(), crate::jobs::DispatchError> {
        self.pending.borrow_mut().push_back(job);
        Ok(())
    }
    fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
}
impl DeferredRecoveryExecutor {
    pub(crate) fn run_next(&self) -> JobOutcome {
        let job = self.pending.borrow_mut().pop_front().expect("queued recovery job");
        job.execute()
    }
    pub(crate) fn pending_len(&self) -> usize { self.pending.borrow().len() }
}
```

Keep execute and foreground merge separately controllable. Tests feed returned outcomes
through apply_job_outcome, and explicitly run finish_iteration after plugin callbacks.
Add barriers inside journaled IO for pre/post-rename, strict sync, CAS and unlink. Use
finite channels with bounded child-process waits; no permanently blocked test threads.

Red/green: `cargo test -p wordcartel --lib recovery_io`.
Assertions: symlink/FIFO/dir rejected without hang; shared guard survives Buffer-like
owner drop until job clone drops; unsupported distinct from busy; opened read handle
identity stays constant through stat/read/sync; each new parent sync is observable.


### Complete queue-acceptance interface and implementation replacements (P2)

These replace the existing Executor trait and implementation methods in jobs.rs; the
existing Job, JobOutcome, InlineExecutor/ThreadExecutor fields, constructors, worker loop
and Drop remain unchanged. DispatchError is public because Executor is public. Err owns
no rejected Job: it has already been dropped, releasing captured leases before return.
The default dispatch deliberately retains prior fire-and-forget semantics for unrelated
callers; new recovery operations MUST use try_dispatch and handle rejection.

```rust
/// A job could not be accepted by the worker queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The queue has no live receiver or has been shut down.
    Closed,
}

/// Shared foreground-to-worker dispatch and completion interface.
pub trait Executor {
    /// Accept ownership of a job, or release it and report queue closure.
    fn try_dispatch(&self, job: Job) -> Result<(), DispatchError>;
    /// Compatibility entry point for existing fire-and-forget dispatch sites.
    fn dispatch(&self, job: Job) { let _ = self.try_dispatch(job); }
    /// Collect currently ready outcomes without waiting.
    fn drain(&self) -> Vec<JobOutcome>;
}

impl Executor for InlineExecutor {
    fn try_dispatch(&self, job: Job) -> Result<(), DispatchError> {
        let outcome = job.execute();
        self.pending.borrow_mut().push(outcome);
        Ok(())
    }
    fn drain(&self) -> Vec<JobOutcome> {
        self.pending.borrow_mut().drain(..).collect()
    }
}

impl Executor for ThreadExecutor {
    fn try_dispatch(&self, job: Job) -> Result<(), DispatchError> {
        match &self.job_tx {
            Some(tx) => tx.send(job).map_err(|_| DispatchError::Closed),
            None => Err(DispatchError::Closed),
        }
    }
    fn drain(&self) -> Vec<JobOutcome> {
        let mut out = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() { out.push(result); }
        out
    }
}
```

Migrate ALL six existing Executor impls plus the new deferred helper to required
try_dispatch: jobs::InlineExecutor and ThreadExecutor as above; e2e::BenchExecutor
forwards each branch to e.try_dispatch(job); durability_regressions::DeferredExecutor
pushes then Ok; app::CountingSwapExecutor counts accepted checkpoint dispatches then
returns inner.try_dispatch result; app's search_esc_does_not_drain_executor::DrainSpy
forwards inner.try_dispatch, preserving its independent drain counter. No implementation
may implement only dispatch and inherit a fictitious successful try_dispatch default.
Adapt imports to include DispatchError where the type is not fully qualified.

Caller census: live executor producers are save::do_save_to, swap::dispatch_swap_write,
reconcile::dispatch_reconcile, and lenses::dispatch_pos_sweep. The old swap producer becomes recovery_flow's checked path. Existing
ordinary-save/reconcile/lens callers can retain compatibility dispatch (their rejection
hardening is outside this bounded change); all NEW recovery scan, prepare, checkpoint,
association and CheckpointAndHandoff submissions use try_dispatch. Registry::dispatch is
a different command API and does not migrate. Job unit-test dispatch calls remain valid.
Do not change FIFO, channel type, worker join, or job outcome transport.

Recovery rejection transitions are synchronous terminal outcomes: scan shows error and
releases its epoch request; prepare marks row failed and advances; ordinary checkpoint
clears only matching provisional/latch state and retains ordinary retry policy;
association/handoff rejection marks protection failure, cancels any waiting quit visibly,
removes concrete blocker and advances selection only if quit is inactive. For initial
handoff rejection AFTER separate Buffer installation, retain that dirty Buffer and its
source on disk, mark it unprotected, remove the request and release the source lease;
no automatic legacy/v2 retirement and no stuck lock waiting for optional retry. Keep the
progress Arc in foreground until try_dispatch returns so rejection reporting does not
need a dropped job. Always distinguish a successful acceptance followed by worker panic
from immediate queue rejection.

Add a rejecting executor (`try_dispatch` explicitly drops job then Err(Closed)) and a
disconnected ThreadExecutor fixture inside jobs tests. Prove no result arrives, captured
guards drop, exact request/latch clears, next selection advances, and an unrelated request
survives. Run both pre-install prepare and installed initial-handoff rejection; reacquire
source from a separate handle/process after rejection to verify release. Check successful
Inline acceptance still routes its queued result correctly with provisional request state.
Red/green: `cargo test -p wordcartel --lib recovery_dispatch` and existing jobs tests.

## Task 2 — V2 codec, ownership and strict checkpoint transaction

Files: new `recovery_store.rs` (split codec into its child module if needed), `lib.rs`.
Proposed worker entry: `checkpoint(fs: &dyn Fs, root: &Path, slot: &RecoverySlot,
record: &CheckpointRecord, body: &str) -> Result<CheckpointAck, RecoveryError>`.
All domain failures are typed and converted to status only in flow.

Encode fixed magic `WCARTEL-RECOVERY\0`, little-endian u32 format version 2, little-endian
u32 metadata length, exact JSON bytes, then exact UTF-8 body. Metadata cap 65536, body
cap MAX_OPEN_BYTES; checked arithmetic before allocation. Reject trailing bytes and
invalid owner/generation/body-count/path tags. Timestamp is Unix milliseconds with a
validated nonnegative range; unavailable clock is explicit unknown, never monotonic
civil time. Tagged paths encode Unix bytes or Windows UTF-16; foreign tags display
losslessly escaped context but never yield an automatic local target. Include lineage,
edit version, association, provenance and optional predecessor owner/generation.

Allocate owner directory exclusively, create/lock stable inode before publishing.
Scanner observing incomplete allocation must return unavailable; never adopt it.
Write exclusive temp 0600 in that owner, write+flush+sync, close, rename checkpoint,
strict sync owner and, until this slot has its first Ack, the full resolved ancestor
chain through filesystem root regardless of which entries appeared newly provisioned. Temp failure cleanup is worker
only. Acknowledgement requires all syncs. Keep old acknowledged generation on failure;
consume the attempted generation regardless. Retain all pending provisioning sync paths
in slot state after any failure, including failures AFTER rename. On retry with the SAME
slot, rerun the complete retained sync set even if every directory now exists; successful
individual syncs do not clear a subset before the final sequence succeeds. The owner
checkpoint-directory sync is always required, independent of provisioning obligations.
On a newly reconstructed store after restart, initialize conservative sync obligations
for the full resolved chain from owner through recovery-v2, configured state root and
all ancestors through filesystem root, regardless of path existence. No serialized Ack or mere existence proves
an interrupted creation durable. Existing source lock acquisition does not adopt its
owner as a writable slot: reconstructed-chain tests exercise successor provisioning and
store reinitialization with already-existing protocol ancestors, never reuse source-owner
identity. Never unlink owner.lock or owner directory.
Keep the slot guard alive for every write and cleanup. No destructor sync/remove/retry.

Red/green: `cargo test -p wordcartel --lib recovery_store`.
Test exact cap body + maximum header accepted, cap+1 rejected, corrupt/trailing/unknown
format preserved, lossless non-UTF8 filename roundtrip; collision retries/no tombstone
reuse; separate unnamed/same-path slots write independent bytes (R1/D4). Inject each
write/flush/sync/rename/ancestor-sync failure: no Ack before final sync, retained prior
copy/source, no generation reuse after uncertain rename. Add failure-then-retry tests for EVERY
ancestor boundary after rename with the same live slot, then repeat with a different
slot and reconstructed store over existing directories. Also fail mkdir BEFORE any
record/slot publication, then retry with a new slot against the partially created root. Journal must show the owed ancestor sync again before
Ack/retirement, not just first-attempt refusal. Assert zero IO on slot creation.

## Task 3 — Worker discovery, legacy compatibility and revalidation

Files: `recovery_store` discovery child; `swap.rs` legacy parsers/policy helpers.
Proposed worker APIs: `scan(fs: &dyn Fs, root: &Path, scope: &ScanScope)
-> Result<Vec<RecoveryCandidate>, RecoveryError>` and
`prepare(fs: &dyn Fs, root: &Path, token: &SelectionToken)
-> Result<PreparedRecovery, PrepareError>`.

Enumerate all entries using list_dir(cap=None) + raw_name. For v2 validate owner dirs,
try existing stable lock, read one capped self-describing record, derive preview, release
scan lease before examining next source. Busy entries remain unavailable manual rows,
not automatic interruptions. Missing/incomplete/corrupt/unsupported candidates and scan
errors stay visible. Empty tombstones do not become recoverable rows. Never scan on ticks.

Legacy .swp uses its own header, not recomputed path hash. Separate bounded header
allowance from MAX_OPEN_BYTES body; decode old text format conservatively. Panic
recovered-*.md is body-only copyable legacy with unknown provenance. Unknown/malformed
files remain preserved/disabled. Sort validated wall time descending; label legacy time
unknown or explicitly filesystem fallback; stable owner/raw-path tie break.

Prepare re-locks one v2 source, rereads and verifies owner+generation. Legacy rereads
raw-path/length/mtime/full-body hash discriminator; changed content returns refreshed
candidate requiring explicit new selection, including same-size changes. No legacy
unlink/rename anywhere in import. Source lease moves into successful result; all failure
paths release it. No holding two existing source leases and no batch body accumulation.

Red/green: `cargo test -p wordcartel --lib recovery_discovery`.
Cover every format, all candidates not just newest scratch, busy locks, nonexistent
original, non-UTF8 raw name, oversized/error distinction, same-size legacy replacement,
changed v2 generation, scan retaining metadata/preview rather than all full bodies.

## Task 4 — Recovery requests, normal checkpoint migration and save receipts

Files: `editor.rs`, `jobs.rs`, `jobs_apply.rs`, new `recovery_flow.rs`, `swap.rs`,
`timers.rs`, `save.rs`, `session_restore.rs`, `workspace.rs`, `lib.rs`.

Initialize memory-only recovery state in Buffer::from_text and Editor::new_from_text;
Buffer::from_file delegates as today. Preserve slot on path changes; replace slot on
whole-buffer replacement, including reload retaining BufferId. Editor::replace_buffer
and workspace::close_buffer_now call cancellation for old BufferId+slot before removal.
Queued jobs retain old slot, and old merges compare both identities. Scratch restoration
constructs a fresh independent slot; permanent scratch gets its own.

Keep swap::dispatch_swap_write as thin delegation to proposed
`recovery_flow::dispatch_checkpoint(ctx: &mut Ctx, id: BufferId) -> DispatchOutcome`.
Capture snapshot/generation/id/slot. Create a provisional exact request entry with
its progress handle BEFORE try_dispatch (Inline executes the job before returning), but
publish the matching in-flight latch only on Ok. On Err terminalize that exact request,
release prepared result/source ownership and clear its provisional state; no completion
will arrive. Report rejection synchronously. Do not leave a fake queued request for a
quit timeout to eventually clean up. Worker uses store checkpoint; merge acknowledges matching slot/request
and content/association generation only. Kind Recovery(id) survives panic; update all
exhaustive JobKind matches in jobs::is_stale, jobs_apply::apply_panic and test counters.
SwapWrite may be removed after migrating its constructors/tests; no live old-format writer
remains. Ordinary failure retries retain current policy; import failure is distinct.

Save captures slot/snapshot/version/association at do_save_to. In the SAME worker closure
as file::save_atomic_with_fs (for Saved AND Unchanged), if the slot owns a checkpoint,
read checkpoint under owner guard, demand exact body and destination association and
no newer generation than the eligible capture. Open committed resolved target once;
compare full capped bytes to captured content, compare its same-handle fingerprint to
committed operation, sync SAME handle, strictly sync destination parent. Only now create
and immediately consume private RecoverySaveReceipt to remove that owned checkpoint;
strictly sync owner dir after unlink. Any failure retains checkpoint where possible and
returns cleanup warning alongside successful save. Unlink-sync failure can leave uncertain
absence backed by durable destination. No receipt may retire a predecessor handoff source.
Ordinary save failure creates no receipt. Foreign/legacy records are never candidates.

Remove all path-derived swap deletes in do_save_to merge and reload_from_disk. Remove
prior_key recovery cleanup capture (retain other valid path/session metadata). Accepted
Save As merge keeps slot and enqueues association intent if dirty or handoff outstanding.
Dispatch that intent using the current snapshot/generation; clean state with no outstanding
handoff creates no old-body checkpoint. A stale old-path save merge keeps current-path
saved metadata unchanged (existing R8 invariant). Save-then-close may drop Buffer before
recovery result; held lease/result still routes safely. Do not turn cleanup warning into
failed Save or reopen a completed quit save prompt.

Red/green: `cargo test -p wordcartel --lib recovery_ownership` and
`cargo test -p wordcartel --lib durability_regressions`.
R1: two unnamed, permanent scratch, same-path buffers/process slots; save one preserves
others byte-for-byte. FIFO checkpoint→save→new checkpoint, save→checkpoint, Save As→late
old-path save; changed/current association and newer/divergent generation preserved.
Fault each receipt stage for Saved and Unchanged, same-handle identity and target change;
ordinary save still successful with visible retained-recovery warning. Closed/reloaded
same BufferId cannot acknowledge old slot; foreign panic clears no other request.

## Task 5 — Separate-document import and atomic predecessor handoff

Files: `recovery_flow`, `recovery_store`, `jobs_apply`, `workspace`, `prompts`, `save`.
Proposed entry points: `begin_selected(ctx: &mut Ctx, tokens: Vec<SelectionToken>)`,
`after_job(ctx: &mut Ctx)`, `cancel_buffer(editor: &mut Editor, id: BufferId)`,
`on_panic(editor: &mut Editor, request: RecoveryRequestId, message: &str)`.

Prepare merge stores a ready intent ONLY. In BOTH apply_job_result/apply_job_outcome,
construct Ctx and call after_job immediately after low-level merge, before quit drain
re-drive. Validate request still live, no quit started, batch not cancelled. Install a
fresh Buffer with alloc_id, path=None, saved_version=None and separate slot/provenance.
Always push it; never reuse disk throwaway or permanent scratch. Derive initial state
normally; preserve disk Buffer. Capture initial body before callbacks. Enqueue one
CheckpointAndHandoff transaction before queueing recovered plugin Open with no fake path.
Do not use load_recovered's old replacement semantics or emit disk Close for this import.

Combined worker transaction owns source lease + successor slot + initial body + progress.
Strictly checkpoint that captured initial body. Set durable flag, CAS Pending→Retiring,
and ONLY if it wins unlink exact locked v2 checkpoint + strictly sync source owner dir.
Legacy source always survives. This entire transaction precedes later jobs for that Buffer
on existing FIFO executor. No deferred cleanup merge/job, historical Ack, or reacquiring
source path. Drop leases at operation end even on failure/panic. Source lock file stays.
Foreground result changes protection/status, removes exact request, then starts next
selection. Terminal handling follows design F table without inferred broad cancellation.

Pre-sync failure: source survives, Buffer visibly unprotected, batch advances; no idle retry.
Post-durable panic/error: successor survives, report uncertain cleanup, do not claim all
protection lost. Repeat unchanged opened candidate focuses live Buffer; failed protection
retry captures CURRENT content and preserves predecessor regardless of byte divergence.
Edit retries ordinary protection. Closing its Buffer removes opened-live association but
not discovered sources; a future fresh import can perform a new initial-body handoff.

Save suggestion: prompts::open_save_as_picker uses recovered suggestion when path=None;
`<stem>-recovered.md` or `recovered-untitled.md`. Use original parent only if available as
validated by picker listing; otherwise existing normal directory fallback. First Save,
manual Save As and quit-owned Save As all seed it; existing resolve/overwrite validation
remains authoritative. Never fabricate document.path before successful Save As merge.

Red/green: `cargo test -p wordcartel --lib recovery_handoff`.
D1/D2/D5 and R5: no further edit needed; real checkpoint dispatched before plugin Open
mutation/save; disk buffer unchanged; overwrite confirmation on occupied suggestion.
Fault every matrix boundary; cancellation before/after CAS; panic progress distinction;
old BufferId/slot late merge; two selected imports sequential through handoff; repeated
failed import focuses/retries without predecessor deletion. Assert surviving on-disk body
at each boundary, not merely status flags. Test both inline and deferred execution.

## Task 6 — Selection overlay, command and all opening paths

Files: new `recovery_picker.rs`, `overlays.rs`, `registry.rs`, `app.rs`, `e2e.rs`,
`workspace.rs`, `session_restore.rs`, `prompts.rs`, `prompt.rs`, `editor.rs`, `swap.rs`.
Proposed flow hooks: `bootstrap(ctx: &mut Ctx)`, `after_callbacks(ctx: &mut Ctx)`,
`opened(editor: &mut Editor, id: BufferId, path: &Path)`, `review(ctx: &mut Ctx)`.

Replace app::run's legacy assess/delete/single-orphan staging. After executor and wake
relay initialization, before first receive, bootstrap one all-candidate scan, including
named launches and clean --no-splash startup. Bootstrap never installs or pumps plugins.
workspace::open_as_new_buffer additive branch and session_restore::open_into_current
throwaway branch enqueue assessment after successful install; picker and recents converge
there. Pure Buffer constructors never scan. Normalize associations through existing path
resolution semantics; result cannot replace edited/switched disk Buffer. Coalesce same
open request without losing distinct candidate tokens. Closed original remains manually
reviewable. Runtime startup is all-candidate even if open intents coalesce into it.

In app::finish_iteration call after_callbacks unconditionally BEFORE !editor.quit early
return and before quit::after_callbacks. It dispatches scan/open/association intents and
deferred safe-boundary offers only, never prepared installation or plugin Open. Both
Harness::step and step_timed use same hook; async ready installation is next job wrapper.
Cancel scan increments epoch; stale results release resources and never offer UI.
Migrate `e2e.rs::Harness::drain_jobs` from apply_outcome to apply_job_outcome with
self.ex, TestClock(self.now), self.tx and self.fs. Migrate its three direct plugin-save
outcome loops too, using the same Fs supplied at dispatch. Exercise plugin pumps followed
by finish_iteration in the shared harness; do not reentrantly pump inside after_job.
Audit direct low-level callers in save.rs, session_restore.rs, file_browser_commit.rs,
swap.rs and file.rs: any scenario expecting association/import follow-up must use the
production wrappers and explicit callback boundary. Leave isolated low-level bookkeeping
unit tests in jobs_apply/reconcile/lenses intentionally context-free, documenting that
they do not certify complete recovery flows. The app pending-save synthetic merge test
may remain low-level if it tests only that synthetic state transition.

Registry registers review_recovery_files / Review Recovery Files… / File, no default key.
Palette exhaustive/menu subset uses same command. No separate plugin bypass. Add Recovery
OverlayId, table row, render-order frame entry and closure/mouse/intercept delegates. One
geometry function controls paint/hit testing. Preserve background JobDone routing; input
handler must not swallow durability completion. Busy, empty, errors visible. No implicit
selection. Up/Down navigate; Space toggle; Enter selected; Enter none says Select a recovery
file and stays open. Esc/mouse click-away/registry close share dismissal function.

Dismiss all displayed available exact tokens, or unselected tokens on subset acceptance;
successful imports record opened token. Automatic filters dismissed/opened tokens, manual
ignores dismissal and can focus live opened Buffer. Changed token is eligible again. A
refreshed/failed row requires explicit selection. Automatic UI waits for no incompatible
overlay; manual command during quit or save/close prompt refuses visibly and preserves flow.

Remove obsolete PromptAction Recover/DiscardSwap/OpenOriginal and swap_recovery constructor,
pending_swap_body/pending_swap_path fields and old load_recovered API after migrating all
callers/tests. Legacy codec/cleaner remain. Update old prompt rendering fixtures to selector
fixtures. Old recovery tests asserting replacement/deletion become separate-buffer retained-
source tests; preserve diagnostics correctness by asserting disk diagnostics unaffected and
new recovered Buffer starts fresh. Old reload provider-close tests remain for reload only.

CleanRecovery protection adds every open document whose path names a recovered dump, every
legacy source referenced by pending/open imports and all v2 owner dirs. Both initial list
and confirmation recheck use same protection set. Review selection grants no delete power.
No generic cleaner redesign or broad legacy deletion protocol in this task.

Red/green: `cargo test -p wordcartel --lib recovery_picker`,
`cargo test -p wordcartel --lib recovery_open`,
`cargo test -p wordcartel --lib overlays`, `cargo test -p wordcartel --lib registry`.
D3/D6/R4: startup named/unnamed/no-input, runtime additive/throwaway/picker/recents, save disk
while scan pending retains candidate; no-edit manual command dispatches real worker. Test
all close routes, multi-select, zero-select, reopen, stale epoch, safe modal deferral and
plugin registry close. Add actual rendered picker tiny/normal terminal and mouse hit tests;
menu/palette completeness + active-keymap hint conformance are final suite requirements.

## Task 7 — Quit, timeout, process crashes and complete regression migration

Files: `quit.rs`, `timers.rs`, `recovery_flow`, `app.rs`, `e2e.rs`,
`recovery_regressions.rs`, existing safety tests in `durability_regressions.rs`.

Add concrete recovery request blockers (handoff/association, not scan or indefinite
unprotected flags) to quit readiness. While quit active reject prepared installations,
cancel remaining selected batch; allow already dispatched combined jobs to finish.
Use after_callbacks before quit final barrier; preserve dirty rescan and explicit discard
versions after plugin edits. A blocking failure/panic visibly cancels waiting quit once;
terminal request stops blocking and next selection can advance only if quit inactive.

Timers expose nearest five-second recovery pending-work deadline through existing timer
registration seam. Timeout attempts Pending→Cancelled, detaches exit blocker but keeps
request/progress for safe eventual completion; cancels current quit once with exact status
Recovery IO is still pending; quit cancelled. Later completion cannot reinstate quit or
install cancelled work. Later Quit follows normal worker join; this does not bound OS IO
or shutdown time. No thread detach or discarded queued saves. Close/reload/quit Discard
cancel pending CAS and batch; dispatched job drains normally. Successful combined job ends
concrete wait and lets normal Save All/Review Each policy assess remaining dirty buffers.

Migrate tests/callers using named-symbol census (run rg before deleting APIs):
`swap_path|swap::delete|delete_with_fs|dispatch_swap_write|JobKind::SwapWrite|pending_swap_|load_recovered|swap_recovery|PromptAction::Recover|DiscardSwap|OpenOriginal`.
Known sites: app SSD-wear tests (counter formerly SwapWrite + delete cleanup), swap module
legacy/write tests, save recovery diagnostics/provider tests, prompts orphan-delete test,
render prompt fixture lists, editor staged fields, jobs_apply swap panic test, durability
regressions old-path checkpoint fixture. Keep legacy parser tests as legacy, migrate live
IO assertions to slot-owned v2 and exact request identity. Do not merely delete coverage.

Process tests launch current test executable with an ignored helper selected by exact
name; parent supplies temp root and channel/file handshake. Test independent processes
same named/unnamed inputs, exclusive collision, active lock unavailable, process death
releases lock, old slot guard held by queued work, and two recovery attempts contending for
one abandoned source. Crash helper at finite controlled write/rename/sync/retire boundaries;
restart scanner verifies source or successor. Kill only the owned helper process. Never
claim these tests simulate power loss; journal tests prove ordering, subprocesses prove
process-death behavior. Windows cfg compilation/runtime evidence is required for any
Windows durability claim; if unavailable state unvalidated and exercise Unsupported
preserve behavior, never mint success from skipped sync.

Red/green: `cargo test -p wordcartel --lib recovery_quit`,
`cargo test -p wordcartel --lib recovery_process`,
`cargo test -p wordcartel --lib e2e`.
Add real Lua callbacks editing/saving on recovered Open; prove initial handoff queued
first and quit rescans. Test prepare arriving during quit, save completion before recovery
merge, timeout then finite release, foreign panic, close/reload with delayed work, explicit
discard, failure releases lock and queue progresses. Idle settled buffers enqueue no new
checkpoint/scan work. Constructor-only tests remain IO-free.

## Final coverage and independent gates

| Requirement | Required evidence |
| --- | --- |
| D1 separate recovered document | Task 5 disk/throwaway preservation, fresh BufferId/slot, one pathless Open; Task 7 plugin e2e |
| D2 first Save As | Task 5 suggested name, unavailable parent fallback, occupied target confirmation, quit-owned picker |
| D3 selection/preservation | Task 6 all cancellation routes, subset/reopen/zero selection; Task 3 stale tokens |
| D4 ownership / R1 | Tasks 2/4 independent slots, save receipts; Task 7 real competing processes |
| D5 handoff / R5 | Task 5 every fault/CAS boundary + retry; Task 7 process-crash successor discovery |
| D6 review command / R4 | Task 6 no-input bootstrap, every open family, manual palette/menu, late scan vs disk save |

Run once final source stabilizes:

```sh
cargo test --workspace
cargo build --workspace
cargo test --workspace --no-run
cargo clippy --workspace --all-targets
scripts/smoke/run.sh
git diff --check
```

Record all counts and warnings; quote smoke summary verbatim (advisory). Workspace tests
include fs_chokepoint, module_budgets, backlog and command-surface invariants. No cargo fmt.
Keep original R1/R4/R5 defect reproductions as historical evidence; new regressions assert
correct behavior rather than claiming old defect-asserting passing probes are fixes.

Independent Codex reviews this PLAN against source before any task implementation.
Fold every Critical/Important/Minor plan finding and re-review to zero. During authorized
execution, per-task reviewer returns two verdicts. Final independent Codex pre-merge and
Fable whole-change review both inspect complete final snapshot; Fable may compile isolated
probes only at that final gate. Repair findings and rerun affected tests and reviews. Record
source hashes, gate logs and reviewed diff so a later merge cannot silently differ.

Report unsupported platform durability and existing FIFO join limitation candidly. R3
checkpoint scheduling, R7 uncooperative external writers and legacy cleaner redesign remain
separate. Do not claim whole-app recovery perfection. User explicitly authorizes any
future commit/merge/push; this plan makes none automatic.

## Revision ledger

- Revision 1: independent plan NO-GO, P1/P2/P3 Important and P4 Minor.
- Revision 2: P1 retained ancestor-sync obligations plus same-slot/reconstructed-store
  retries; P2 observable dispatch acceptance and exact rejection cleanup with complete
  Executor method replacements; P4 safe cached rustix effective-UID source. P3 retains
  its original finding and is governed by the scoped format decision linked above.
  This revision awaits re-review and does not claim a clean implementation gate.
