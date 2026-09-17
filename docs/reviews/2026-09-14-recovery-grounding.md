# Recovery safety source grounding — 2026-09-14

Read-only source investigation at `9e409c0f09d9683c40e84dbd2499c9f9e6e32ded`. This is an architectural map, not an independent review of the proposed design. No cargo commands or implementation edits were performed by this investigator. Symbol names below are the navigation anchors.

## Ownership and persistence

| Source / symbol | Current behavior | Design consequence |
| --- | --- | --- |
| `wordcartel/src/swap.rs::swap_path` | Named documents share a canonical-path hash key; every unnamed buffer in a process shares `scratch-PID.swp`. | Neither path nor PID distinguishes editing instances. Same-document windows and unnamed buffers collide. |
| `editor.rs::DocumentId` / `Buffer::from_text` | A minted 64-bit lineage **hint**, explicitly not a uniqueness invariant; every fresh buffer constructor mints one. | Do not upgrade this hint into deletion authority. Introduce separately validated recovery ownership. |
| `swap.rs::SwapHeader`, `serialize`, `parse` | Version-1 line header includes lossy string path, content hash, edit version, monotonic clock timestamp, PID and optional opaque lineage. Unknown keys are ignored. | Legacy path association and timestamp provenance are imperfect. A new wall-clock timestamp cannot be inferred from old `ts_ms`. Preserve unknown/malformed legacy artifacts. |
| `swap.rs::dispatch_swap_write` | Captures active buffer id/version/path and snapshot; worker writes; durability completion updates the originating buffer if its derived path still matches. | New operations need exact ownership and request/generation identity, not an active-buffer or current-path lookup. Closed/replaced buffers can still have workers writing. |
| `swap.rs::write_atomic_with_fs` | Atomic replacement fsyncs the file but passes `dir_fsync: false`. | Existing success is insufficient proof for deleting the predecessor after a power-loss-safe handoff. |
| `fsx.rs::atomic_replace` / `RealFs::sync_dir` | Rename then optional directory sync; directory-open errors in existing `sync_dir` are swallowed. | A strict durability primitive must propagate directory-open and sync failures and cover newly created parent-directory entries. Do not silently change ordinary-save contracts. |
| `jobs.rs::ThreadExecutor` / `is_stale` | One FIFO worker; durability completions are never dropped by the generic stale-result gate. | FIFO applies within one process, not between processes. Foreground cleanup may run while later FIFO work has already executed; ownership guards must outlive buffer close and protect queued operations. |

`timers.rs::on_tick` and `swap_deadline` only inspect the active buffer. `swap::due` uses the latest edit as the first-checkpoint max baseline, so uninterrupted initial typing can postpone the first checkpoint. These are separate known scheduling issues; immediate recovered-document checkpointing cannot depend on this timer. `on_tick` sets `swap_in_flight` before dispatch, while dispatch can return early if state-directory provisioning fails; a new explicit dispatch outcome should avoid inheriting that stranded latch. `jobs_apply::apply_panic` presently clears the swap latch by buffer id alone.

## Every production open family

- `app.rs::run`: constructs launch buffer using `Buffer::from_file`, installs permanent scratch, restores persisted scratch, then performs the only recovery assessment. Named launches call `swap::assess`; identical hash causes immediate path-derived deletion. Unnamed launch calls `find_orphan_scratch_swap`, which selects one newest legacy scratch candidate from a dead process. Recovery is staged on the launch buffer before terminal initialization. A named launch does not discover unrelated orphan unnamed work.
- `file_browser.rs::file_browser_enter`: normal picker and recents selections converge on `workspace::open_as_new_buffer`.
- `workspace::open_as_new_buffer`: reuses a clean unnamed throwaway via `session_restore::open_into_current`, otherwise constructs and pushes a new buffer, restores resume state, rebuilds and fires the plugin Open event. Neither branch assesses recovery.
- `session_restore::open_into_current`: allocates a fresh `BufferId`, replaces through `Editor::replace_buffer`, restores resume state, repairs MRU, fires Open. Fresh ids protect its slot from old jobs' foreground writes; they do not prevent those jobs' disk writes.
- `editor.rs::Buffer::from_file`: injected document read; missing file becomes a clean named new document. Constructor alone does not have executor/message/prompt context. Recovery assessment should be an application-level lifecycle seam, shared by both additive and throwaway opens and startup, rather than an interactive side effect hidden in this constructor.
- `session_restore::restore_scratch`: separate persisted-scratch installation, intentionally independent of file resume settings. A recovered unnamed draft must be a separate ordinary document and must not overwrite this permanent stash.
- `save.rs::reload_from_disk`: user-authorized whole-buffer replacement, retaining `BufferId`, bumping document version, resetting metadata and deleting path-derived swap. It is not currently an ordinary open-event path. Design must state whether reload preserves the same editing-instance owner or explicitly retires it; stale workers remain possible either way.

Plugin Open callbacks occur after installation and can change active buffers or content. Any recovery discovery result needs captured buffer/path plus an epoch; stale results must not open a modal over an unrelated interaction or silently disappear from the later review command. Selection-list dismissal, Escape, mouse dismissal and overlay-registry closure all require the same non-destructive semantics.

## Cleanup and replacement census

| Site | Current action / trap |
| --- | --- |
| `save.rs::dispatch_save` completion | Successful current-version save deletes current path-derived swap; Save As also deletes dispatch-time `prior_key`. Save As while newer edits exist still deletes the prior key. Both may remove other owners' work; saved version alone cannot justify deleting a newer checkpoint. |
| `save.rs::reload_from_disk` | Deletes current path-derived swap after successful replacement. Old jobs retain the preserved `BufferId`; version bump protects diagnostics, not all durability effects. |
| `save.rs::load_recovered` | Replaces active document with recovered text, retains its path and buffer id, marks unsaved, but installs fresh buffer scheduling fields. It does not immediately checkpoint. This contradicts the approved separate-document/Save-As behavior and must be replaced in the live recovery path. |
| `prompts.rs::PromptAction::Recover` | Removes orphan source after calling `load_recovered`, without a durable successor acknowledgement. Even refusal by the replacement chokepoint does not prevent the subsequent remove. |
| `prompts.rs::DiscardSwap` / `OpenOriginal` | Discard directly deletes orphan or derived swap path. Open Original only drops staged references, but a subsequent save can still delete that preserved path. New normal saves must have no authority over discovery candidates. |
| `workspace.rs::close_buffer_now` / quit discard | Closing does not delete swap; last ordinary close creates a fresh unnamed throwaway. Retain this conservative behavior unless the approved design explicitly changes it. Dropping a buffer is not proof that its worker or recovery handoff finished. |
| `swap.rs::cleanable_recovery_files`, `recovery_path_still_cleanable`, `prompts.rs::CleanRecovery` | Snapshot then per-path recheck, followed by remove. This is conservative for many legacy swaps but is not an atomic interprocess compare-and-delete. New ownership locks must cover verification and removal. |
| `swap.rs::recovery_file_is_cleanable` | Any unprotected `recovered-*.md` dump is considered deletable. Do not put new protected draft records under this legacy naming rule. |
| `swap.rs::open_swap_paths` | Protection is computed from current buffer paths, not source candidates or in-flight handoffs. Discovery, selected sources and handoff leases need independent protection. |
| `recovery.rs::write_dump`, `dump_all_dirty`, `dump_on_panic` | Emergency plain-text dumps use basename/PID/sequence names; panic latch stores only the last edited snapshot. They are a separate legacy artifact class, with less metadata than swaps. Discovery must explicitly include or document their handling. |

## Technical decisions the design must make explicit

1. A per-owner directory and lifetime lock can separate writers, but never unlink/recreate its lock inode while another process might hold or acquire it. Unknown lock failures mean preserve, not assume abandonment. Keep the guard alive through dispatched writes, cleanup, handoff and panic completion—not just as a field on a live buffer.
2. Legacy writers will not honor new locks. Copy-only import with no automatic source deletion is a defensible compatibility rule; state clearly that it preserves the approved handoff guarantee conservatively by retaining extra old copies. Do not claim locking makes a legacy scan/read/delete race safe.
3. If the new record's first write succeeds but its source retirement fails, both remain reviewable. If the recovered buffer saves or closes before that write completes, the source-to-successor proof must still be reconstructible. Never delete the only durable successor merely because the document is now clean while a predecessor retirement is still pending.
4. Store the original path as recovery provenance separate from `Document.path`. Leave `Document.path = None` until Save As succeeds. `prompts::open_save_as_picker` currently derives directory from path and always passes an empty filename; it needs a separate suggested destination for recovered drafts.
5. Timestamp display, stable row identity, bounded preview reads, multiple selections, malformed/unreadable/oversized candidates and repeat selections all need contracts. A record changing after enumeration requires revalidation; do not open cached partial content or silently interpret read failure as an empty recovered document.
6. The new command must be registered once through registry/palette/menu contract, and its overlay must participate in shared keyboard, mouse and plugin close routes. Avoid growing central dispatchers with scan or recovery mechanics.

## Required regression and fault coverage

- Two unnamed ordinary buffers plus permanent scratch; two same-path buffers; two processes; Save As from one owner; save/reload/close from another: independently recoverable bytes survive.
- Open via startup, picker additive branch, picker throwaway reuse and recents; original version stays available, selected recoveries become separate dirty unnamed buffers, plugin Open mutations do not redirect recovery application.
- Recover without any later edit; checkpoint dispatch and completion are real; first Save and Save-and-Quit suggest a distinct destination and use existing overwrite confirmation.
- Failed create, write, flush, file sync, rename, strict directory open/sync, read, parse and source removal. Verify durable surviving bytes after each boundary, not just latches or success messages.
- Delayed completion after edit, Save As, ordinary save, reload, close, active switch, quit, and worker panic; replay/stale completion cannot retire another record or clear another request's in-flight marker.
- Two processes discover/recover/discard the same abandoned v2 record; lock contention, owner death, PID reuse, and collision retry preserve ownership. Locked live records are not recovery candidates for mutation.
- Legacy same-path swap replaced after enumeration; legacy copy-only import never deletes it. Legacy malformed/oversized/unknown-format files survive; panic dumps are handled explicitly.
- Selection list reopen, cancellation through every input/overlay path, zero selections, multiple selections, unselected retention, stale scan epoch and repeated import; no dismissal performs disk mutation.
- Ordinary idle behavior remains write-free once settled; immediate handoff bypasses periodic scheduling without adding repeated idle writes. Existing source-level fs chokepoint, module budget and command conformance tests remain applicable.
