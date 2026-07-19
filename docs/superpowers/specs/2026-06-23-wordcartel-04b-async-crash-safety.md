# Wordcartel Effort 4b — Async Edges & Crash Safety (Design Spec)

**Date:** 2026-06-23
**Status:** Design approved (brainstorm) — pending plan
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§3.9, §10.3, §10.4, §14.3, §15, §18.4)
**Predecessor:** Effort 4a (synchronous terminal shell, merged `d94fc39`)
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

---

## 1. Goal

Move slow IO off the keystroke path and make the parent spec's **"never lose the
user's work"** guarantee (§15.1) real — **without changing the synchronous core**.
4a shipped a usable editor whose every operation, including save, runs on the
foreground thread. 4b introduces the async substrate the rest of the editor's
slow work will ride on, converts save to a background job, and adds the crash /
panic / external-modification safety net.

This spec is **deliberately scoped to the substrate and the safety net.** The
"platform" layer (filter primitive, repar transforms, pandoc export, system
clipboard) is split out into **Effort 4c**; config / keymap / palette / spellcheck
/ mouse / nav / wrap-guide remain **Effort 5**. See §9 Non-Goals.

## 2. Architecture at a glance

Two independently-shippable sub-plans:

- **4b-1 — Async substrate + background save + dispatch registry.** The job system
  (general, plugin-ready per §18.4), the main-loop rewrite that lets a finished job
  wake the UI, background save with a version/dirty model, and migration of 4a's
  command dispatch onto a name-keyed registry (the §10.4 boundary). Folds in the
  cheap 4a polish items.
- **4b-2 — Crash safety.** Periodic swap/recovery file, panic-time buffer dump,
  external-modification detection, and the shared modal-prompt mechanism that
  serves quit-confirm, external-mod, and recovery.

**No new `repar` dependency.** 4a already implemented and hardened an atomic save
(`wordcartel/src/file.rs::save_atomic`: same-dir O_EXCL temp, fsync, rename,
dir-fsync, symlink refusal, mode preserve, skip-unchanged, `TempGuard`). 4b reuses
it over a rope snapshot. The `repar` library dependency (§14.1/§14.3) lands in
**Effort 4c** with the Reflow/Unwrap/Ventilate transforms that actually need it.

## 3. Global Constraints (inherited; bind every task)

- **Responsiveness is #1** (§3.9): the foreground thread never blocks on IO. p95
  keystroke < 16 ms. `status` is written *before* a job is dispatched (instant
  feedback), never after.
- **Functional core / imperative shell** (§10, §14.4): `wordcartel-core` stays
  IO-free and thread-free. All threading, IO, and OS calls live in the `wordcartel`
  shell crate.
- **Reconcile = discard, not rebase** (§10.3): stale job results (version moved on)
  are dropped. No OT/CRDT.
- **Never lose work / never crash silently** (§15.1–15.2): the on-disk `.md` is
  never half-written; failures surface with immediate, non-blocking feedback;
  modal only for genuinely destructive decisions.
- **Single mutation channel** (§10.1): all **document text / history** mutation
  still flows through `editor.apply`. Handlers and job merges may touch non-document
  state directly (selection, view, status, register, `saved_version`, prompt/quit
  flags) — those already mutate outside `apply` in 4a — but a job's only way to
  change the **document** is by returning a `merge` whose body calls `editor.apply`.
  In 4b every shipping `merge` (`Save`, `SwapWrite`) touches **only** status /
  `saved_version` / stored-fingerprint bookkeeping and never the document text; a
  `merge` that mutates document text without going through `apply` is a contract
  violation even though the `FnOnce(&mut Editor)` type permits it.
- **LF-only line semantics** (matches core's `TextSource`): the shell counts lines
  by `\n` only; bare `\r` and U+2028/U+2029 never split a line.
- **Plugin substrate (§18.4):** the job API must stay general enough to host a
  future plugin-invoked transform; the dispatch boundary must be key→ID→handler so
  the closed enum never becomes the extensibility boundary.

---

## 4. Sub-plan 4b-1 — Async substrate + background save + registry

### 4.1 The job substrate (general / plugin-ready)

The substrate lives in the shell crate (e.g. `wordcartel/src/jobs.rs`). Its only
consumer in *this* spec is background save (4.3) plus the swap write (5.1), but its
contract is shaped for every future slow job (spellcheck, full-document search,
filters, plugin transforms).

**Types (contract; exact field names are normative):**

```rust
/// A unit of background work, dispatched from the foreground for a given
/// document version, run on a worker thread, merged back on the foreground.
pub struct Job {
    /// Document version at dispatch time — the staleness token.
    pub version: u64,
    /// Classification used for staleness + coalescing policy.
    pub kind: JobKind,
    /// The work. Captures whatever it needs (an O(1) rope snapshot, a path).
    /// Runs on the worker thread; must not touch the Editor directly.
    pub run: Box<dyn FnOnce() -> JobResult + Send>,
}

/// What a worker hands back. The worker returns *its own merge logic* as a
/// foreground effect — maximally general: any future job plugs in unchanged.
pub struct JobResult {
    pub version: u64,
    pub kind: JobKind,
    /// Applied on the foreground thread before the next draw. The ONLY way a
    /// job mutates editor state (keeps §10.1's single-writer invariant). The
    /// type permits arbitrary `&mut Editor` access, but by contract a merge
    /// touches only non-document bookkeeping (status, `saved_version`, stored
    /// fingerprint); any document-text change must route through `editor.apply`.
    /// In 4b both shipping kinds (Save, SwapWrite) are bookkeeping-only.
    pub merge: Box<dyn FnOnce(&mut Editor) + Send>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum JobKind {
    Save,       // one-shot, user-initiated: always applies
    SwapWrite,  // one-shot housekeeping: always applies (status only)
    #[cfg(test)]
    CoalesceProbe, // test-only stand-in for a future coalescible kind
    // future (4c/5/P): Spellcheck, Search, Filter, Plugin(...) — coalescible
}
```

The coalescible **staleness/latest-wins path is real code in 4b** but has no
shipping caller; `CoalesceProbe` (compiled only under `#[cfg(test)]`) exercises it
so the discard-when-stale and single-slot behavior is covered now (§7) rather than
deferred to 4c.

**Staleness policy (per kind):**
- **One-shot kinds** (`Save`, `SwapWrite`): the `merge` always applies (the user
  asked, or it is pure housekeeping). They never carry stale document state because
  their effect is status/`saved_version`, not buffer content.
- **Coalescible kinds** (future): drain-time guard discards the result when
  `result.version != editor.document.version`, and dispatch uses a latest-wins
  single slot per kind so only the newest snapshot is worked (§10.3 debounce).

**v1 implementation:** a **single worker thread** consuming an mpsc `job_rx`. A
single worker gives **FIFO result ordering for free**, which removes any
out-of-order `saved_version` regression hazard (4.3). The latest-wins single-slot
machinery is specified now but exercised only when the first coalescible kind
arrives (4c); for 4b's one-shot jobs a plain channel send suffices. Detailed queue
mechanics are an impl-spec item for the plan.

**Testability — `Executor` trait:**

```rust
pub trait Executor {
    fn dispatch(&self, job: Job);          // enqueue for the worker
    fn drain(&self) -> Vec<JobResult>;     // non-blocking: collected results
}
```

- Production: `ThreadExecutor` (spawns the worker thread, `drain` = `try_recv` loop).
- Tests: `InlineExecutor` runs `job.run()` immediately on `dispatch` and buffers
  the result for `drain` — **deterministic, no real threads, no flake** (the same
  injection pattern as 4a's `Clock`). Every substrate behavior (staleness, FIFO,
  save transitions) is unit-tested through `InlineExecutor`.

### 4.2 Main-loop rewrite — waking on job completion

**Problem:** 4a's `app::run` blocks on `crossterm::event::read()`. A background job
that finishes mid-block would not surface (status "Saved", merged result) until the
*next* keypress — a silent wait, violating §3.9.

**Solution — unified channel + input-reader thread:**
- A dedicated **input thread** loops on `crossterm::event::read()` and forwards each
  event into a unified `mpsc::Sender<Msg>`.
- **Lifecycle:** `event::read()` blocks indefinitely, so the input thread cannot be
  cleanly joined on quit. It is **detached** and allowed to die with the process: on
  `editor.quit` the main loop breaks, restores the terminal, and returns from
  `main`; the orphaned `read()` is reaped at process exit. The thread holds no locks
  and owns no un-flushed state, so abandoning it is safe. (A future switch to
  `event::poll` + a shutdown flag is possible but unneeded in 4b.) A send on the
  unified channel after the receiver is dropped is a benign `SendError` the input
  thread ignores.
- The worker's results and a periodic timer tick feed the **same** channel.
- The main loop becomes:

```rust
enum Msg { Input(crossterm::event::Event), JobDone(JobResult), Tick }

for msg in rx {                       // blocks only when truly idle; zero CPU spin
    match msg {
        Msg::Input(ev)   => handle_event(ev, &mut editor, &clock),
        Msg::JobDone(r)  => apply_result(r, &mut editor),
        Msg::Tick        => on_timer(&mut editor, &clock),   // swap cadence (5.2)
    }
    drain_results(&executor, &mut editor);     // fold any others ready
    guard.terminal().draw(|f| render::render(f, &editor))?;
    if editor.quit { break; }
}
```

- **Instant wake** on job completion, **zero idle CPU**, the render loop still never
  blocks. `Tick` is driven by `recv_timeout` to the next swap deadline (5.2) rather
  than a busy poll.
- This is a localized change to `app::run`; `app::step` stays pure and the existing
  `step` unit tests are unaffected.

### 4.3 Background save + version/dirty model

- Replace 4a's bool `dirty` with **`saved_version: Option<u64>`** on the document.
  `dirty == (Some(self.version) != self.saved_version)` — `None` means never saved
  (new/scratch). This makes "is there unsaved work?" a function of versions, not a
  flag that can drift.
- **Save flow (Ctrl+S):**
  1. Foreground: write `status = "Saving…"` immediately (§3.9).
  2. Foreground: **external-mod check** — `stat` the target (microseconds, cheap
     enough to stay sync) and compare against the stored `Option<FileFingerprint>`
     (§5.4); on mismatch, **do not dispatch** — raise the external-mod modal prompt
     (§5.4) instead. A worker thread cannot prompt, so the decision stays on the
     foreground.
  3. Foreground: capture `snap = buffer.snapshot()` (O(1)), `v = version`, and the
     owned `path`. Dispatch a `JobKind::Save` job. `save_atomic` takes
     `(path: &Path, content: &str)` (4a's signature, unchanged), so the **worker**
     materializes the rope snapshot to a `String` (`snap.to_string()`) and then
     calls `file::save_atomic(&path, &content)`. Materialization happens on the
     worker, off the keystroke path; the foreground only does the O(1) snapshot.
  4. Worker: atomic save of the materialized snapshot. Returns a `merge` that, **on
     success**, sets `saved_version = Some(v)` and refreshes the stored fingerprint
     (§5.4) **unconditionally** (those record what is now on disk at version `v`),
     but sets `status = "Saved"` **only if `editor.document.version == v`** — i.e.
     the buffer is now clean. If the user edited on to `v+1` while the save was in
     flight, the buffer is still dirty, so the status becomes `"Saved v<n>"` /
     remains a dirty indicator rather than a misleading bare `"Saved"`, and the swap
     file is **not** deleted (§5.1). **On failure**, the merge leaves
     `saved_version` and the stored fingerprint untouched (buffer stays dirty), the
     original file untouched (§14.3), and surfaces the error in the status line
     inviting retry / save-as.
- **Ordering:** the single worker is FIFO, so two in-flight saves (V then V+2) merge
  in order; `saved_version` never regresses. The `== v` clean check above means a
  later in-flight save can still mark the buffer clean once its own version matches
  the current document. (Save is user-initiated and rare; no coalescing is applied
  to it.)
- **Skip-unchanged** (§15.3) is preserved by `save_atomic` (no inode churn; quiet
  no-op report).

### 4.4 Command dispatch registry (the §10.4 boundary, brought forward)

- Introduce a **name-keyed registry**: `CommandId` (interned string) → `Handler`.
  Bindings become **key → `CommandId`**; dispatch is registry lookup → handler.
  Built-in commands register at startup.
- **Handler signature** mirrors §10.4 (`fn(&mut Ctx) -> CommandResult`); handlers
  route all **document text / history** changes through `apply` (via the effects /
  transactions they produce) and may touch non-document state (selection, view,
  status, register, quit/prompt flags) directly, exactly as 4a's commands already
  do — `move_*` updates selection/view, `copy`/`cut` update the register and status,
  `quit` updates quit/prompt state. The invariant is single-writer over the
  **document**, not over all editor state.
- **Printable keys keep the §10.4 literal-insert fallthrough**: an unmapped
  printable key inserts literal text. This removes the "command needs an argument"
  problem — every *named* command is arg-less (`save`, `undo`, `redo`, `copy`,
  `cut`, `paste`, `move_left`, `move_right`, `move_up`, `move_down`,
  `move_line_start`, `move_line_end`, `cycle_render_mode`, `quit`, …); character
  insertion is the default path, not a registered command.
- **Scope guard:** 4b builds the registry *mechanism* and migrates 4a's existing
  commands **behavior-preservingly** — every one of 4a's 84 shell tests must stay
  green. 4b does **not** build config-file parsing, user-overridable keybindings, or
  the command palette; those remain Effort 5 and register/resolve through this same
  boundary. The point of doing it now is that 4b is the last moment before a wave of
  new commands (4c/5) would otherwise grow the closed enum into the de-facto
  extensibility boundary §10.4 forbids.

### 4.5 4a polish (folded into 4b-1)

Carried forward from the 4a deferral list (ledger row 4a):
- **undo/redo no-op robustness:** undo with empty history is a true no-op — it must
  not mark the buffer dirty and must not reset `desired_col`.
- **CycleRenderMode → `ensure_visible`:** a render-mode change can change layout and
  scroll; call `ensure_visible` afterward so the caret stays on screen.
- **Copy-on-empty guard:** copying with an empty selection must not overwrite the
  register with `""`.
- **LF-only line model:** replace the shell's use of ropey's unicode-aware
  `len_lines`/line APIs with LF-only counting, consistent with core's `TextSource`;
  bare `\r` / U+2028 must not split a logical line.

---

## 5. Sub-plan 4b-2 — Crash safety

### 5.1 Swap / recovery file

- **Purpose (§15.7):** a vim-`.swp`-style periodic snapshot that **never overwrites
  the real `.md`**, offered for recovery on next open. Together with atomic save
  (half-write protection) and the panic dump (unwind path) it covers crash, panic,
  and power loss.
- **Location:** `$XDG_STATE_HOME/wordcartel/` (fallback `~/.local/state/wordcartel/`)
  via `etcetera`/`dirs`. One swap per open document.
- **Permissions (Unix):** swap and panic-dump files contain the user's **full
  document text and absolute realpaths**, so they must not be world-readable. Create
  the state directory `0700` and every swap / temp / final / panic-dump file `0600`
  (set the mode at create time on the O_EXCL temp, before rename, so there is no
  window at a wider mode). On non-Unix the platform default applies.
- **Filename:** `<sanitized-name>-<hash(realpath)>.swp`; scratch/unnamed buffers use
  `scratch-<pid>.swp`. The path hash disambiguates same-named files in different
  directories and avoids embedding path separators.
- **Content:** a small textual header followed by the full buffer text:
  - header fields: format version, original realpath (absent for scratch),
    original `(mtime, size)` *at load* (absent if F did not exist at load), a
    **content hash of the buffer text** (the recovery predicate's primary key,
    §5.1 recovery-on-open), document version, wall-clock timestamp, pid.
  - written **atomically** (temp + rename) **on a worker** as a `JobKind::SwapWrite`
    job — the first non-save consumer, which validates the substrate's generality.
- **Cadence (decided): idle-debounce + max-interval cap.**
  - Write the swap `T_idle` seconds after the last edit (default **2 s**) — matches
    prose rhythm; no churn while idle.
  - Force a write at least every `T_max` seconds during continuous editing (default
    **30 s**) so an uninterrupted burst can't lose everything.
  - Only while `dirty`. Both tunables are constants in 4b (config exposure = Effort
    5). The timer rides the unified channel's `Tick` (4.2) via `recv_timeout` to the
    next deadline.
- **Lifecycle:** deleted on clean quit, and on a save **only when that save leaves
  the buffer clean** — i.e. the save result's version equals the current document
  version (`saved_version == Some(document.version)`; the same `== v` check as
  §4.3). A background save that completes for an older snapshot while the user has
  edited on leaves the buffer dirty, so the swap is **kept**, not deleted. Recreated
  on the next edit; **survives a crash**.
- **Recovery-on-open:** when opening file F, compute its swap path. A bare timestamp
  compare is unreliable (clock skew, same-size replacement, atomic-rename mtime), so
  the predicate is **content-hash first, stat as a tiebreaker**:
  1. If no swap file exists → open F normally (no prompt).
  2. If the swap's `header.content_hash` equals the hash of F's current bytes → the
     swap adds nothing; **silently discard** it and open F (no prompt).
  3. Otherwise the swap diverges from F → **raise the prompt** (§5.3):
     **[R]ecover into buffer · [D]iscard swap · [O]pen original (ignore swap)**.
     This covers: F missing now but present in the swap header (recover offered);
     F present but changed since the swap's recorded `(load mtime, size)`
     (concurrent external edit — recover lets the user keep their in-flight work);
     and scratch/unnamed swaps (no F to compare — always prompt if the swap is
     non-empty).
  The swap header therefore stores a **content hash of the buffer text** in addition
  to the load-time `(mtime, size)`, so divergence does not depend on wall-clock
  ordering. Recover loads the swap content into the buffer (marked dirty, original
  path retained). A best-effort pid-liveness check warns "file may be open in
  another instance." Three-way merge is backlog (§15.3).

### 5.2 Timer mechanics

- The main loop computes the next swap deadline from `last_edit_at` and
  `last_swap_at` and uses `rx.recv_timeout(next_deadline - now)`. A timeout yields
  `Msg::Tick`; an event/result simply re-arms. No dedicated timer thread needed.
- The swap write is dispatched as a job (never inline on the foreground), so a slow
  state-dir filesystem can never introduce typing lag.

### 5.3 Modal-prompt infrastructure (shared)

> **AMENDED by C5 (2026-07-19).** The "single-line" half of this section now reads: the
> **question and its choices** are a single line, painted on the status row. A prompt may
> additionally carry a typed `detail: Vec<String>` — structured disclosure painted as a
> bordered box directly ABOVE the status row (`chrome_geom::prompt_detail_rect` +
> `render_overlays::paint_prompt_detail`). See C5 §11.3 for the reasoning and the standing
> constraints; the three 4b prompts below are unaffected and carry an empty `detail`.

A small reusable modal mechanism (e.g. `wordcartel/src/prompt.rs`) renders a
single-line/centered prompt with labeled choices and routes the keypress to a
result. Three users in 4b:
- **Quit with unsaved changes** — upgrades 4a's double-Ctrl+Q to a real prompt:
  **[S]ave & quit · [Q]uit anyway · [C]ancel**. On "save & quit," dispatch the save
  and then **wait for that save's specific `JobResult`** (match on its `version`),
  not a blanket worker join — a slow earlier job must not extend the wait. The wait
  is **bounded** by a timeout (default **5 s**, a constant in 4b): on success, delete
  the swap (§5.1) and exit 0; on save **error**, abandon the quit and return to the
  prompt with the error in the status line (the user can retry or choose
  "Quit anyway"); on **timeout**, do **not** exit silently — re-raise the prompt
  noting the save is still running so the user explicitly chooses to keep waiting or
  quit anyway (the swap file remains as the safety net). This is the one place a
  brief, user-initiated, clearly-labeled "Saving…" wait is acceptable.
- **External modification** (§5.4): **[R]eload · [O]verwrite · [S]ave as**, with
  exact per-action semantics defined in §5.4.
- **Swap recovery** (§5.1): **[R]ecover · [D]iscard · [O]pen original**.

Modal is reserved for exactly these destructive/ambiguous decisions (§15.2);
everything else stays a transient status-line message.

### 5.4 External-modification detection

- **Fingerprint model — `Option<FileFingerprint>`.** A `FileFingerprint` is
  `{ mtime, size }`. The stored value is an `Option`, because the target may not
  exist:
  - **Existed at load** → `Some(fp)` captured at load.
  - **New named buffer / did not exist at load** (`:w` to a fresh path) → `None`;
    "no fingerprint" is itself the expected state, and a *successful* save fills it
    in. A pre-save `stat` that now finds a file where there was `None` means another
    process created it → treat as an external-mod conflict (prompt).
  - **Scratch / unnamed buffer** → `None` and no path; external-mod detection is
    inapplicable until the buffer is given a name (Save-as, Effort 5).
  - **Existed at load, missing at save time** → the stored `Some(fp)` no longer
    matches a present file; surface as external modification (the file was deleted
    out from under us) so the user decides, rather than silently recreating it.
  The fingerprint is captured at load and **refreshed after every successful save**
  (the merge in §4.3), so it always reflects what wordcartel last wrote.
- Before dispatching a save, re-`stat`; compare against the stored
  `Option<FileFingerprint>` per the table above; on mismatch raise the external-mod
  prompt (§5.3) rather than silently clobbering (mitigates atomic-save's
  last-writer-wins, §15.3). Detection only in v1; richer 3-way merge is backlog.
- **Prompt action semantics (§5.3 [R]eload · [O]verwrite · [S]ave as):**
  - **[R]eload** — discard the in-memory buffer and reload F from disk.
    *Destructive to unsaved edits*, so it is only reachable via this explicit modal.
    Resets buffer text, clears undo/redo history, moves the caret to a valid
    position (clamp), refreshes the stored fingerprint to F's current `(mtime,
    size)`, sets `saved_version = Some(new_version)` (clean), and deletes the swap.
  - **[O]verwrite** — proceed with the save the user originally asked for, bypassing
    **only** the fingerprint conflict (last-writer-wins). Re-runs the §4.3 save flow
    skipping the stat check; on success refreshes the stored fingerprint and clears
    dirty as usual.
  - **[S]ave as** — write to a different path. Full path entry is **Effort 5**; in
    4b this action is **surfaced but disabled** (or omitted from the choice list)
    and the prompt notes it is unavailable, so the user picks Reload or Overwrite.

### 5.5 Panic dump

- 4a's panic hook (`term.rs::install_panic_hook`) already restores the terminal
  (leave raw mode, show cursor, disable enhancements). 4b extends it to **dump the
  buffer**:
  - A process-global `Mutex<Option<(Option<PathBuf>, ropey::Rope)>>` ("last good
    snapshot") is updated after each `apply` with the O(1) snapshot + path.
  - The panic hook **`try_lock`s** it (never a blocking `lock()`): a panic that
    fires *while the snapshot is being updated* would deadlock on a blocking lock,
    so the dump is best-effort on contention — if the lock is held, skip the dump
    rather than hang. On success it writes `recovered-<name>-<pid>.md` (mode `0600`,
    §5.1) into the state dir (best-effort; ignore write errors), then prints the
    panic report for a bug filing.
- Shares the state directory and write routine with the swap file. This is the
  unwind-path belt-and-suspenders behind the swap file's periodic protection.

---

## 6. Module structure (shell crate)

- `jobs.rs` — `Job`, `JobResult`, `JobKind`, `Executor` (+ `ThreadExecutor`,
  `InlineExecutor`), worker lifecycle.
- `save.rs` — background-save orchestration + `saved_version`/dirty model (reuses
  `file::save_atomic`).
- `swap.rs` — swap header (de)serialization, atomic write, scan/recover, lifecycle.
- `recovery.rs` *(or fold into `swap.rs` + `term.rs`)* — panic-dump routine + the
  global last-good-snapshot.
- `registry.rs` — `CommandId`, registry, default keymap, dispatch.
- `prompt.rs` — modal-prompt mechanism.
- `app.rs` — unified-channel main loop + input thread + timer.
- Touched: `editor.rs` (saved_version, last-good-snapshot update), `commands.rs` /
  `input.rs` (migrate to registry), `term.rs` (extend panic hook).

## 7. Testing strategy

- **Substrate:** through `InlineExecutor` — staleness discard for the test-only
  `CoalesceProbe` kind, one-shot always-apply, FIFO ordering, `merge` is the only
  document-mutation path.
- **Background save:** dirty/`saved_version` transitions; success path; **version-
  aware status** — a save whose `v` matches the current version marks clean
  ("Saved"), a save that completes after a further edit (`v != current`) refreshes
  `saved_version`/fingerprint but leaves the buffer dirty and keeps the swap;
  failure keeps dirty + file untouched; external-mod intercept at dispatch over the
  `Option<FileFingerprint>` matrix (existed/new/scratch/deleted); skip-unchanged
  no-op; two-saves FIFO ordering.
- **Save & quit:** waits on the matching save `version`; success deletes swap +
  exits; save error returns to the prompt; **timeout re-raises the prompt** (no
  silent exit).
- **Swap:** header round-trip (incl. content hash and `Option` mtime/realpath);
  trigger timing under a fake clock (idle and max-cap paths); **recovery predicate**
  — hash-equal → silent discard, hash-diverged → prompt, F-missing → prompt,
  scratch → prompt; delete-on-clean only when `saved_version == document.version`;
  atomic write leaves no litter on simulated failure; **created `0600` under
  `0700` dir** (Unix).
- **Panic dump:** test the dump *routine* directly given a snapshot (do not actually
  panic in a test); cover the `try_lock`-held path → dump skipped, no hang.
- **Registry:** key→ID→handler dispatch; unknown ID surfaced (never silent no-op,
  §12.5); literal-insert fallthrough; **all 84 4a shell tests preserved**.
- **External-mod:** detection logic with injected `Option<FileFingerprint>`; per-
  action semantics (Reload resets text/history/fingerprint + deletes swap;
  Overwrite bypasses only the stat conflict; Save-as disabled in 4b).
- **Determinism (§11.3):** no test spawns a real thread or sleeps; `Executor` and
  `Clock` are injected. Worker-thread wiring is covered by one focused integration
  test using a deterministic handshake, not timing.

## 8. Risks & mitigations

- **Main-loop rewrite regresses input handling.** Mitigate: keep `app::step` pure
  and its tests intact; the input thread only *transports* events. Cover the new
  loop with an integration test that drives `Msg` variants.
- **Registry migration silently changes a 4a behavior.** Mitigate: migration is
  behavior-preserving by definition — the 84 existing shell tests are the gate; no
  test may be weakened to pass.
- **Swap file races the real file / another instance.** Mitigate: swap never writes
  the `.md`; recovery is an explicit prompt; pid-liveness is best-effort advisory.
- **Background save loses the "Saved" wake.** Mitigate: the unified channel is the
  whole point — a `JobDone` wakes the loop immediately; covered by an integration
  test.

## 9. Non-Goals (explicit deferrals)

- **Effort 4c:** filter primitive (§3.5), repar transforms Reflow/Unwrap/Ventilate
  (§14.1) + the `repar` dependency, pandoc export presets, system-clipboard sync
  (`arboard`/OSC 52, §15.6).
- **Effort 5:** config-file parsing, user-overridable keymap, command palette +
  menu, spellcheck, mouse, word/page navigation, wrap-guide ruler.
- **Backlog:** three-way merge on external modification; autosave directly *to the
  real file* on a timer (rejected for v1, §15.7 — the swap file is the timer-based
  protection, and it never overwrites the user's `.md`).

## 10. Spec → parent-section traceability

| This spec | Parent §           | Item |
|---|---|---|
| §4.1, §4.2 | 10.3               | sync core, async edges over snapshots, version-discard, no tokio |
| §4.3       | 14.3, 15.3         | background atomic save; failure keeps file untouched + dirty |
| §4.4       | 10.4, 18.4         | key→ID→handler registry; plugin-substrate boundary |
| §5.1, §5.2 | 15.7               | periodic swap/recovery file, never overwrites `.md` |
| §5.3       | 15.2               | modal only for destructive/ambiguous decisions |
| §5.4       | 15.3               | external-modification detection + prompt |
| §5.5       | 15.7               | panic-time emergency buffer dump |
| §4.5       | 4a deferral list   | undo/redo no-op, CycleRenderMode visibility, copy guard, LF-only lines |
