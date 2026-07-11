# Effort B — pre-P chores sweep (H12 + M8 + H5) — design spec

Three small, INDEPENDENT chores bundled only to amortize pipeline overhead — they share no code.
Lightweight calibrated pipeline: this spec → one Codex pass → per-chore implementer+reviewer → one
Fable whole-branch gate → merge. Anchors below use symbol NAMES (lines drift; re-locate by name).

## Global constraints (every chore obeys)
- House style: hand-formatted (NEVER `cargo fmt`), em-dashes not `--`, snake_case/PascalCase/SCREAMING,
  private fields + accessors, exhaustive matches, no unwrap on fallible paths, no dead code/dbg!.
- Merge GATEs: `cargo test --workspace` green; `cargo build`/`cargo test --no-run` warning-free for touched
  crates; `cargo clippy --workspace --all-targets` clean (deny); `too_many_lines`/`module_budgets`
  respected; `wordcartel/tests/backlog.rs` green. PTY smoke is mandatory-run / advisory-pass (not a gate).
- **No data loss (H5):** no path may delete a swap that holds recoverable content. Only provably-non-recovery
  artifacts are ever auto-removed.
- **Command-surface contract:** H5 adds one command → §H5 states conformance. M8 + H12 are N/A (no command
  surface touched).

---

## Chore 1 — M8: surface undo-eviction on buffer-level merges (the last M5 follow-up)

**Problem (grounded).** The M5 undo-memory cap (`history.rs::MAX_UNDO_BYTES` = 64 MiB) evicts oldest
revisions in `History::evict_to`, setting the transient `History::last_evicted` count. Today the ONLY hint
surface is `editor.rs::note_undo_eviction(pre_id, pre_version)` (called once per reduce at `app.rs` after
`reduce`), which fires `editor.status = UNDO_EVICTED_HINT` ("Undo history full — oldest dropped") iff the
**active** buffer's id matches `pre_id`, its version changed, and `last_evicted > 0`. So:
- **Active-buffer keystroke evictions → already hinted.**
- **Buffer-level merges** — `ResultClass::BufferLocal` job results merged via `by_id_mut(buffer_id).apply(...)`
  (`jobs_apply.rs::apply_filter_done`, `apply_transform_done`→`transform::merge_transform_into`) — evict too,
  but when the merged buffer is **non-active**, `note_undo_eviction`'s `active()`-only check misses it and the
  eviction is **SILENT**. On the active buffer the merge also sets its own status ("filter applied") which
  `note_undo_eviction` then overwrites — a race, not a lost hint.

**Design.** Replace the active-only check with a **per-buffer eviction signal** so no eviction is silent:
- Add a private field to `Buffer` (editor.rs), e.g. `undo_evicted_pending: u32`, defaulted 0. In `Buffer::apply`
  (the single mutation channel that already calls `commit_coalescing`→`evict_to`), after the commit, if
  `document.history.last_evicted > 0`, set `undo_evicted_pending = document.history.last_evicted` (and reset
  `last_evicted` there, consuming it at the buffer boundary). This captures evictions on ANY buffer, active or not.
- Surface it (the "louder" hint) in two places, consuming the flag when shown:
  1. **After reduce**, for the ACTIVE buffer: if `active().undo_evicted_pending > 0`, set the louder status and
     clear the flag. (Replaces the current `note_undo_eviction` body; keep the fn name + the pre_id/pre_version
     signature is no longer needed — simplify to `surface_undo_eviction(&mut self)` reading the active buffer's
     flag. Update the single call site + the e2e mirrors.)
  2. **On switch to a buffer**: when a buffer becomes active, if its `undo_evicted_pending > 0`, surface the
     louder status + clear — so a non-active merge's eviction is shown the moment the user lands on that buffer.
     **Hook at the single chokepoint `Editor::switch_to_index`** (every user-visible switch funnels through it —
     `workspace::switch_to`, buffer cycling, scratch nav, palette buffer rows, menu document rows, quit-drain
     review — AND `workspace::open_as_new_buffer` calls `switch_to_index` DIRECTLY, bypassing `switch_to`; per
     Codex, surfacing at `switch_to_index` covers all of them without a missed path).
- **"Louder" message.** A buffer-level merge dropping undo depth is more consequential than a keystroke cap, so
  the surfaced string is distinct/more prominent than the plain keystroke case — e.g.
  `UNDO_EVICTED_MERGE_HINT = "Undo history trimmed to fit — some earlier states dropped"` (final wording at
  implementation; it must read as a clear, deliberate notice, not a passing status). The plain keystroke path
  may keep `UNDO_EVICTED_HINT` or unify on the louder one — implementer's call, but buffer-level merges MUST
  surface a hint.

**Invariants.** `undo`/`redo` must NOT set the pending flag (they don't commit; `last_evicted` is already reset
to 0 there). A buffer-switch that finds no pending eviction changes nothing. The flag is per-buffer and cleared
on surface, so it fires exactly once per eviction event.

**Seam:** `Buffer` (editor.rs), `Buffer::apply`, the post-reduce surface call (app.rs) + e2e mirrors, the
buffer-switch/activate path. **Tests:** active keystroke eviction still hints once (port the existing
`editor.rs` tests); a non-active buffer-local merge eviction sets the pending flag and surfaces on switch; an
active buffer-local merge eviction surfaces the louder hint; undo/redo never trigger it; no-eviction merge is
silent. Command-surface: **N/A**.

---

## Chore 2 — H5: `Clean recovery files…` command (safe redesign — NO launch prune)

Decision (locked, revised after Codex spec-gate round 1): **command-only**, and the command deletes ONLY
artifacts a safety oracle (`swap::assess`, or a byte-match to the saved file) proves have **no recovery value**.
There is **NO launch-time auto-prune** — the original "safe `.tmp` prune" was UNSAFE: `fsx.rs::atomic_replace`
does write→fsync→**rename**, so a process that dies between fsync and rename leaves a `.wcartel-{pid}-*.tmp`
holding a COMPLETE, recoverable newest snapshot; a dead pid does NOT prove the temp is partial/valueless. Same
for arbitrary dead-pid `*.swp` (a body differing from the saved file is exactly `RecoveryDecision::Prompt`).
So nothing is ever deleted without the user's explicit, confirmed action AND an oracle verdict of "no value".

### The command
- Register `clean_recovery` in `registry.rs::Registry::builtins` — label `"Clean recovery files…"` (trailing …
  convention for prompt-opening commands, cf. `save_as`), `Some(MenuCategory::File)`. Palette-reachable by
  registry membership; no default keybinding (bindable later).
- **Enumerator (single source of truth)** — a helper (in `swap.rs`/`recovery.rs`) that scans `swap::state_dir`
  and returns the EXACT set of provably-deletable paths:
  - **`recovered-*.md` dumps** — the app's own recovery OUTPUT (already-extracted content the user is explicitly
    choosing to clear via this named command). Included.
  - **`*.swp` that a safety oracle vouches for** — parse the swap's `SwapHeader::realpath` (an
    `Option<String>`; if `None`, EXCLUDE). **Bind `assess` to the exact candidate file:** compute
    `swap::swap_path(Some(realpath))` and require it to EQUAL the scanned candidate path; if it differs (a stale
    or relocated swap not at its canonical location), EXCLUDE — otherwise `assess` would recompute
    `swap_path(doc_path)` and judge a DIFFERENT file than the candidate, and a clean verdict for `B.swp` could
    wrongly greenlight deleting a recoverable `A.swp`. Only when the candidate IS the canonical swap for its
    realpath, run `swap::assess(realpath, …)` and include ONLY on the "no recovery value" `DiscardSilently`-class
    verdict. EXCLUDE `Prompt`-class (recoverable), `OpenNormally`, and any unreadable/unparseable/ambiguous case
    (fail closed).
  - **`.tmp` atomic-write temps** — include ONLY if provably valueless (its bytes equal the current saved file
    at its target, i.e. zero recovery value). Otherwise EXCLUDE (a crash-window `.tmp` may be the newest
    snapshot). When in doubt, exclude.
  - ALWAYS EXCLUDE: any live-pid file (`swap::pid_is_live`), the current session's own swaps, and the swap
    backing any currently-open buffer.
- **Flow (TOCTOU-safe):** the command runs the enumerator ONCE, snapshots the exact `Vec<PathBuf>` into a new
  `Editor` pending field `pending_clean: Vec<PathBuf>` (mirror the existing `pending_after_save`/`pending_save_as`
  cluster), and opens `prompt::Prompt` with a count-bearing message built from that set's len (mirror
  `prompt::close_confirm`/`quit_multi(n)` interpolation, e.g. `"Delete N recovery file(s)? …"`) + a new
  `PromptAction::CleanRecovery` variant (`prompt.rs`). `prompts.rs::resolve_prompt` on `CleanRecovery`+confirm
  deletes EXACTLY the snapshotted `pending_clean` set (best-effort per file; report `"Cleaned N file(s)"`), then
  clears the pending field; on cancel/any-other, clears the pending field and deletes nothing. If the enumerated
  count is 0, set status `"No recovery files to clean"` and do NOT open the prompt. Deleting the snapshot (not a
  re-scan) closes the count≠deleted-set race Codex flagged.
- **Visibility (Codex minor):** `swap::pid_is_live` is private; expose it `pub(crate)` (or keep the enumerator in
  `swap.rs`) so the helper can reuse it — no reimplementation.

**Command-surface conformance.** `clean_recovery` is a new command in the registry → appears in the palette
(law 3) and the File menu (menu ⊆ palette, law 4, via `menu::grouped_commands`). It is an ACTION, not a
user-settable option (law 2 N/A). No multi-state (law 8 N/A). No new keybinding (hints N/A; bindable later).
Conforms.

**Seam:** `swap.rs` (state_dir scan, enumerator, `pid_is_live` visibility, header→doc-path, `assess`),
`recovery.rs` (`recovered-*.md`), `editor.rs` (`pending_clean` field), `registry.rs` (command), `prompt.rs`
(`PromptAction::CleanRecovery` + count message), `prompts.rs::resolve_prompt` (handler). **Tests (via the `Fs`
seam / tmpdir):** enumerator includes a `recovered-*.md` and an `assess`-DiscardSilently `.swp`; EXCLUDES an
`assess`-Prompt (recoverable) `.swp`, a live-pid file, an open buffer's swap, and a `.tmp` whose bytes differ
from its target; confirm deletes exactly the snapshotted set even if a new file appears after the prompt opens
(TOCTOU); cancel deletes nothing; count 0 → no prompt. **No-data-loss asserted:** a dead-pid `.swp` with unsaved
content (`assess`→Prompt) and a crash-window `.tmp` (bytes ≠ saved file) are NEVER included/deleted.

---

## Chore 3 — H12: PTY smoke S9 (live-splash journey)

Additive coverage — the startup splash is exercised by in-process e2e (`e2e.rs`) but NO live-binary smoke check
touches it, because `scripts/smoke/tmux-drive.sh::start_wcartel` hardcodes `--no-splash` on every launch.

### 3a. Infrastructure (two changes, prerequisites)
- **`start_wcartel` splash opt-in:** add a way to launch WITHOUT `--no-splash` — mirror the existing
  `--no-barrier` flag parsing (tmux-drive.sh): a per-launch arg (e.g. `--with-splash`) that omits `--no-splash`
  from the argv. Keep `--no-config` always. (A splash launch also needs `--no-barrier` since the default barrier
  `wait_for '\[1/'` never matches while the splash covers the status row — the check passes `--no-barrier` and
  does its own `wait_for` on splash text, exactly as s8 does for its modal.)
- **Discovery glob:** `run.sh` discovers checks via `checks/s[1-8]-*.sh` — widen to include `s9` (e.g. `s[1-9]`
  or `s*`). Update the "S1–S8" header comments in `run.sh` and `tmux-drive.sh` to S1–S9.

### 3b. The S9 check (`checks/s9-live-splash.sh`)
Follow the existing check anatomy (shebang, `set -eu`, `SMOKE_SOCKET`/`OWN_SERVER`, `mktemp` WORK +
`SMOKE_STATE_HOME`, source `tmux-drive.sh`, `S=s9`, `trap cleanup EXIT`). Body:
1. `start_wcartel s9 --with-splash --no-barrier` (splash up).
2. `wait_for s9 'wordcartel'` and assert the real splash first frame: wordmark `wordcartel`, tagline
   `Everyone needs a cover story`, footer `press any key` (the real strings from `splash.rs`:
   `WORDMARK`/`TAGLINE`/`FOOTER`). Assert nothing-behind (a splash owns the screen).
3. `keys s9` a single key to dismiss.
4. `wait_for s9 '\[1/'` (the status buffer indicator now appears) — the editor is revealed; assert the splash
   text (`press any key`) is GONE.
Mirror the e2e journey `e2e_splash_first_frame_then_key_dismisses_and_is_consumed` for the assertion content.
Keep it to the core journey; the swap-recovery-wins-over-splash variant is already e2e-covered and is OUT of
scope for S9 (may be a later addition). Exit non-zero on any failed `wait_for`.

**Advisory-pass:** S9 runs in the mandatory suite; a red S9 is an advisory finding, not a merge blocker (same
as S1–S8). **Seam:** `scripts/smoke/tmux-drive.sh` (`start_wcartel`, glob comment), `scripts/smoke/run.sh`
(glob), new `scripts/smoke/checks/s9-live-splash.sh`. Reference: `splash.rs` (real strings),
`e2e.rs` splash journeys. Command-surface: **N/A**.

---

## Sequencing & gates
Three independent tasks (any order; suggested M8 → H5 → H12). Each: TDD (failing test → impl → green →
commit), implementer + per-task reviewer (spec + quality). Then ONE Fable whole-branch gate (logic changes in
M8/H5 warrant it; H12 is script-only) → `--no-ff` merge. Codex gates this spec once (sole spec gate). Backlog:
mark H12/M8/H5 shipped at merge.
