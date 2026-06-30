# M4-rest: parse-panic isolation + input-thread supervision — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-06-30
**Effort:** M4-rest (completes the M4 panic-isolation campaign before Effort P)

## Context

The pre-plugin hardening campaign's M4 effort isolated panics on the job worker
and the transform/filter/export ad-hoc threads (shell `panicx::catch` boundary),
but explicitly deferred two failure modes (`term.rs:92` documents the gap):

1. **An upstream `pulldown-cmark` 0.13.4 panic** (`parse.rs:2199`,
   `Option::unwrap()` on `None`) reachable from *any* markdown parse. M7's F2 fuzz
   oracle surfaced it. All block-tree parsing runs **synchronously on the main
   thread** (centralized in `derive::rebuild`), so this panic crashes the whole
   app — and because the panic hook is gated to the main thread, it would tear
   down the terminal — on the most common user action (typing a character).
2. **Reader-thread death hangs the app.** The input reader thread is detached and
   its `Sender<Msg>` clone is one of several; if it dies (a panic, or
   `crossterm::event::read()` returning `Err` because the controlling terminal
   went away), `msg_rx` never disconnects, so the main loop silently hangs —
   visually alive, totally unresponsive, terminal still in raw/alt-screen mode.

This effort closes both. It is **entirely shell-side** and reuses existing
machinery (`panicx::catch`, the `recovery` dump, the `Msg` channel). The
functional core (`wordcartel-core`, `#![forbid(unsafe_code)]`) is untouched, with
one possible trivial exception called out below.

## Goals

- A markdown parse panic must **never** crash the app or corrupt the terminal,
  and must **never** lose document text (parsing is read-only).
- Input-thread death must produce a **clean, deterministic shutdown** (terminal
  restored, unsaved work dumped to recovery) instead of a silent hang.
- A dead clipboard worker must surface a status notice instead of silently
  failing pastes.

## Non-goals (explicitly out of scope)

- Patching/upgrading `pulldown-cmark` (we **isolate** the panic, not fix upstream).
- Restarting any helper thread (input death → shutdown, not respawn).
- Wake-relay supervision (a dead wake-relay only delays job application to the
  next timeout — it already degrades acceptably).
- The deeper incremental-block-tree soundness work (deeper nested/loose-list
  incremental≡full divergences) — its own future effort.
- Any new async/deferred parse path; parsing stays synchronous on the main thread.

## Existing machinery reused (verified signatures)

- `wordcartel/src/panicx.rs:5` — `pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String>`
  (wraps `catch_unwind(AssertUnwindSafe(f))`, maps the payload to a `String`).
- `wordcartel/src/recovery.rs` — the panic-time recovery dump invoked by the hook
  (`recovery::dump_on_panic`, called at `term.rs` in `install_panic_hook`). The
  plan confirms the exact entry to call on a graceful input-loss shutdown (reuse
  `dump_on_panic`, or a sibling non-panic dump if one is cleaner).
- `wordcartel/src/term.rs:96` — `install_panic_hook()`; `term.rs:80`
  `should_handle_panic(panicking, main) -> bool` (main-thread-gated). The terminal
  restore sequence (`disable_raw_mode` + `LeaveAlternateScreen` + show cursor) used
  by the hook is the same teardown the input-loss path must run.
- `editor.status: String` — the status-line field, set by mutating it in `reduce`
  or its callees.

## Goal 1 — parse-panic boundary

### Seams (all shell-side, verified)

All block-tree parsing flows through three call sites:

| File:line | Call | Thread | Path |
|---|---|---|---|
| `derive.rs:95` | `block_tree::incremental_update_rope(&editor.active().document.blocks, &old_rope, &edit, &new_rope)` | main | **hot** (per-keystroke) |
| `derive.rs:104` | `block_tree::full_parse_rope(&new_rope)` | main | cold (load/undo/redo) |
| `editor.rs:127` | `block_tree::full_parse_rope(&buffer.snapshot())` in `Buffer::from_text` | main | cold (buffer creation) |

`derive::rebuild` computes `new_blocks` from one of the two branches (95 or 104)
and assigns it at `derive.rs:107`:
`editor.active_mut().document.blocks = new_blocks`.

### Design

Factor `rebuild`'s "compute the new tree" step into a single guarded operation so
a panic from **either** branch is caught at one point:

```
let prev = editor.active().document.blocks.clone();   // cheap: BlockTree is the prior state, already owned
let computed: Result<BlockTree, String> = panicx::catch(|| {
    // the existing branch logic: incremental_update_rope(..) or full_parse_rope(..)
});
match computed {
    Ok(tree) => { editor.active_mut().document.blocks = tree; clear_parse_degraded(editor); }
    Err(_msg) => { /* keep prev: do NOT overwrite document.blocks */ set_parse_degraded(editor); }
}
```

(The `clone()` of the previous tree is only needed if the borrow of
`editor.active().document.blocks` inside the closure conflicts with the later
mutable borrow; the plan picks the minimal borrow-safe shape — e.g. read the
inputs out first, then `catch`, then assign — to avoid an unnecessary clone on the
hot path. The hot path must stay `O(visible)+O(edited)`; the only added cost on a
**successful** parse is the `catch_unwind` frame, which is negligible.)

**Fallback trees:**
- `derive::rebuild`: **reuse the previous `BlockTree`** (do not overwrite
  `document.blocks`). Preserves folds and outline state; the worst case is stale
  block roles for the edited region until the next successful parse. Consumers
  (`role_at`, fold/nav/transform/outline) tolerate a tree whose spans lag the
  buffer — they return defaults for out-of-range offsets, never panic.
- `Buffer::from_text` (`editor.rs:127`): there is **no** previous tree, so fall
  back to an **empty Document-root `BlockTree`** (a `Document` root spanning
  `0..len`, no children → every `role_at` returns the default `Paragraph`). Use an
  existing public empty/plain constructor if `wordcartel-core` exposes one;
  otherwise add a trivial **pure** `block_tree` constructor (e.g.
  `pub fn empty_tree(len: usize) -> BlockTree`) — the only candidate core change,
  and it introduces no `unsafe` and no parsing.

**Deduped status (Q1).** Add app-level parse-degraded state (a `bool`, e.g.
`parse_degraded` on the run-loop/App state):
- `Err` while `!parse_degraded`: set `parse_degraded = true` and
  `editor.status = "markdown parse failed — styling may be stale"`.
- `Err` while already `parse_degraded`: do nothing (no per-keystroke spam).
- `Ok` while `parse_degraded`: set `parse_degraded = false` and clear the notice
  by setting the status to empty (the default idle state) — it does **not** try to
  restore a prior unrelated message; only the degraded notice it owns is cleared.
- `Ok` while `!parse_degraded`: normal, no change.

The flag tracks the active buffer's most recent rebuild outcome; switching buffers
triggers a rebuild that re-evaluates it. No new `Msg` variant is needed — the
parse is synchronous, so the `Err` is handled inline in `rebuild`.

## Goal 2a — input-thread supervision

### Current state (verified)

- `app.rs:1959` `let (msg_tx, msg_rx) = mpsc::channel::<Msg>();`
- `app.rs:1979–1986` input thread spawn (`wcartel-input`), `JoinHandle` discarded;
  loop: `while let Ok(ev) = crossterm::event::read() { if msg_tx.send(Msg::Input(ev)).is_err() { break; } }`.
- `app.rs:2089` `msg_rx.recv_timeout(timeout)` → `Timeout` ⇒ `Msg::Tick`,
  `Disconnected` ⇒ `break`. Disconnect never fires while clipboard/wake-relay
  clones live, so input death hangs.

### Design

1. **New variant:** `Msg::InputThreadDied` (added to the `Msg` enum at `app.rs:26`).
2. **Keep the handle + watchdog.** Capture the input thread's `JoinHandle` and
   spawn one tiny watchdog thread (`wcartel-input-watchdog`) holding a `msg_tx`
   clone:
   ```
   let _ = input_handle.join();                 // unblocks on ANY input-thread end (Ok or panic)
   let _ = watchdog_msg_tx.send(Msg::InputThreadDied);
   // watchdog exits
   ```
   No false positive on normal quit: on quit the input thread is still blocked in
   `read()` (uninterruptible), so `join()` stays blocked and the watchdog is reaped
   by process exit — `InputThreadDied` is sent only on a genuine death.
3. **Shutdown path.** The main loop, on `Msg::InputThreadDied`, breaks carrying a
   distinct exit reason. Introduce a small `ExitReason` the loop returns to `run()`:
   `Normal` (existing quit/disconnect) vs `InputLost`. `run()` inspects it:
   - always run the existing terminal teardown (restore from raw/alt-screen);
   - for `InputLost`: additionally invoke the recovery dump
     (`recovery::dump_on_panic` or its non-panic sibling), print a one-line reason
     to stderr (e.g. `"input reader stopped — terminal may have closed; recovery
     written"`), and exit non-zero.
   Factoring the decision into `ExitReason` keeps it unit-testable without real
   thread teardown.

This complements (does not change) the main-thread panic hook: an input-thread
panic is handled by the watchdog → main-loop path (the main thread does the
restore + dump), so the hook stays main-thread-only and we never restore the
terminal from a non-main thread (which would race concurrent terminal writes).

## Goal 2b — clipboard-death notice (Q4 → B)

The clipboard-intent drain (`app.rs` ~45–48) already detects a dead clipboard
worker: `clip_tx.send(ClipReq::Get{..})` returning `Err` synthesizes a fallback
`Msg::ClipboardPaste { text: None }`, and `clip_tx.send(ClipReq::Set(..))` is
best-effort-ignored. At that existing `Err` detection point, additionally set
`editor.status = "clipboard unavailable"` so a silently-failing copy/paste is no
longer mysterious. ~1–2 lines; no watchdog, no new `Msg`. The clipboard `Get`
fallback-to-`None` behavior is unchanged.

## Error-handling summary

| Failure | Response | Data loss? | Terminal | Process |
|---|---|---|---|---|
| Markdown parse panic | deduped status, reuse previous tree (empty tree on first parse) | none (text untouched) | intact | keeps running |
| Input thread death | terminal restored + recovery dump + stderr reason | none (dump persists work) | restored | exit non-zero |
| Clipboard worker death | status "clipboard unavailable" | none | intact | keeps running |

## Testing

- **Parse boundary (deterministic):** factor the guarded compute so a test can
  pass a **panicking closure** in place of the real parse and assert: previous
  tree reused (unchanged `document.blocks`), `parse_degraded` set, status text
  set; then a succeeding closure clears `parse_degraded` and the notice. A
  `Buffer::from_text`-style test asserts the empty-tree fallback on first-parse
  panic. *Best-effort:* if the real `pulldown-cmark` panic input is cheaply
  reproducible, pin it as an end-to-end regression test (drive `rebuild` with that
  text, assert no crash + status set); not a blocker if it can't be minimized.
- **Input watchdog (deterministic):** spawn a thread that returns immediately, run
  the watchdog join+send logic, assert `Msg::InputThreadDied` arrives on the
  channel. Separately, a unit test that the loop given `Msg::InputThreadDied`
  yields `ExitReason::InputLost`, and that `InputLost` drives the dump + non-zero
  exit decision (test the decision function, not real teardown).
- **Clipboard notice:** drop the clipboard `Receiver`, call the intent drain with
  a `Get` (and a `Set`) intent, assert `editor.status == "clipboard unavailable"`
  and the existing `text: None` fallback still fires.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact recovery-dump entry to call on graceful `InputLost` shutdown
   (`recovery::dump_on_panic` vs a non-panic variant) and how `run()` currently
   tears the terminal down on normal exit (so `InputLost` reuses it + adds dump).
2. Whether `wordcartel-core` already exposes a public empty/plain `BlockTree`
   constructor; if not, add the trivial pure `block_tree::empty_tree(len)`.
3. The borrow-safe shape of the guarded `rebuild` step that avoids cloning the
   previous tree on the successful hot path.
4. Exact `Msg` enum location/derives and the `reduce`/loop site that must match
   `Msg::InputThreadDied`, plus where `parse_degraded` state lives (App vs Editor).
