# M4-rest: parse-panic isolation + input-thread supervision — design

**Status:** spec-review clean (Codex READY FOR PLANNING, no open findings)
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

This effort closes both. It is **almost entirely shell-side** and reuses existing
machinery (`panicx::catch`, the `recovery` dump, the `Msg` channel). The only
functional-core (`wordcartel-core`, `#![forbid(unsafe_code)]`) change is one
trivial **pure** constructor — `block_tree::empty_tree(len)` — needed for the
parse-panic fallback (no `unsafe`, no parsing; `Block`/`BlockTree` fields are
already public, so it is a thin wrapper).

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
  This effort extends `panicx` with a **thread-local caught-panic guard** (see the
  panic-hook interaction in Goal 1) so a main-thread caught panic suppresses the
  hook's teardown.
- `wordcartel/src/recovery.rs` — two entries: `dump_on_panic()` (`:30`, best-effort
  `try_lock` of `LAST_GOOD` — a **single** most-recently-edited snapshot; used by
  the panic hook, unchanged here) and `write_dump(path, rope, dir) -> io::Result<PathBuf>`
  (`:18`, writes a 0600 `recovered-*.md` for **an arbitrary buffer's** rope). The
  input-loss shutdown uses `write_dump` **per dirty buffer** (not `dump_on_panic`,
  which would persist only one) so no open buffer's unsaved work is lost.
  `swap::state_dir()` (`swap.rs:31`) gives the dump directory.
- `wordcartel/src/term.rs:96` — `install_panic_hook()`; `term.rs:80`
  `should_handle_panic(panicking, main) -> bool` (main-thread-gated). Normal
  teardown (terminal restore) is RAII via `TerminalGuard::drop` (`term.rs:65`).
  The hook also runs the restore + `dump_on_panic` on a real main-thread panic.
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

### Panic-hook interaction (the load-bearing detail)

`catch_unwind` does **not** stop the installed panic hook: Rust runs the hook *at
the panic site, before* the stack unwinds and before `catch_unwind` returns. So a
caught **main-thread** parse panic would still fire `term.rs`'s hook — which
restores the terminal and calls `dump_on_panic` — defeating "catch and keep
running." (Latent today only because every existing `catch` site is on a non-main
thread, where `should_handle_panic` already returns `false`.)

**Fix — a thread-local caught-panic guard in `panicx`:**
- A thread-local flag (e.g. `CAUGHT_GUARD: Cell<bool>`). `panicx::catch` wraps its
  `catch_unwind` in a **save/restore RAII guard** (`prev = flag.replace(true)`;
  `Drop` sets `flag = prev`) — re-entrant-safe — established *before* the
  `catch_unwind` so the flag is still `true` when the hook runs at the panic site.
- The hook (`term.rs`) consults it: teardown only when
  `should_handle_panic(current, main) && !panicx::caught_guard_active()`.
- Effect: a panic *inside* a `panicx::catch` closure on the main thread suppresses
  the hook's teardown (the `catch` owns the failure); a genuine uncaught
  main-thread panic still triggers the hook normally. Worker threads are unaffected
  (already gated off). This makes `panicx::catch` correct on the main thread, which
  it was not before.

### Design

Factor `rebuild`'s "compute the new tree" step into a single guarded operation so
a panic from **either** branch (95 or 104) is caught at one point. On `Ok`, assign
the new tree and clear the degraded flag; on `Err`, install the **empty-tree
fallback** and set the degraded flag:

```
let new_len = new_rope.len_bytes();
let computed: Result<BlockTree, String> = panicx::catch(|| {
    // the existing branch logic: incremental_update_rope(..) or full_parse_rope(..)
});
let tree = match computed {
    Ok(t)  => { clear_parse_degraded(app);              t }
    Err(_) => { set_parse_degraded(app, editor);        block_tree::empty_tree(new_len) }
};
editor.active_mut().document.blocks = tree;
```

The hot path stays `O(visible)+O(edited)`: the only added cost on a **successful**
parse is the `catch_unwind` frame (negligible). No previous-tree clone is needed.

**Fallback tree — empty-tree, uniformly (Q1 revised → A).** On any caught parse
panic, set `document.blocks` to an **empty Document-root `BlockTree`** spanning
`0..new_len` with no children. This is used at **all three seams** (both
`derive::rebuild` branches and `Buffer::from_text`). Rationale: an empty tree has
**no child spans**, so the span-slicing consumers (fold / outline / nav /
transform) cannot slice the current rope out of range — eliminating the
secondary-panic hazard that reusing a now-stale previous tree (whose spans lag the
mutated buffer) would create. `role_at` returns the default `Paragraph` everywhere.
Cost: during a parse-panic episode the document transiently shows no heading
styling / folds / outline; all of it is restored on the next successful parse. The
empty tree spans `0..new_len` so even its single root span matches the current
rope. Requires the trivial pure core constructor `block_tree::empty_tree(len)`.

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
   This obliges updates the spec must account for: the **manual `Debug` impl**
   (`app.rs:65–103`) gets a new arm, and the **main exhaustive `match msg`**
   (`app.rs:1651–1756`) gets a new arm. **Routing:** the modal/overlay match
   (`app.rs:1403–1442`) uses a catch-all `_ => {}` that would *swallow*
   `InputThreadDied` while a prompt/overlay is open — so the variant MUST be
   handled at the top of message dispatch, **before** any modal/overlay
   interception, so the shutdown fires regardless of UI state.
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
3. **Shutdown path + exit code.** The main loop, on `Msg::InputThreadDied`, breaks
   carrying a distinct reason. Introduce a small `ExitReason` { `Normal`,
   `InputLost` } that the loop produces and `run()` returns to `main()`. Sequence
   (ordering matters because `std::process::exit` runs **no** destructors):
   - On the `InputLost` break, before returning, **dump every buffer holding
     unsaved work** (persist it — independent of the terminal): iterate the
     editor's open buffers (`Editor::buffers`, `editor.rs:287`) and for each whose
     **raw `Document::dirty()`** (`editor.rs:66`) is true, write a recovery dump via
     the existing `recovery::write_dump(buffer.document.path.as_deref(), &rope, &dir)`
     (`dir = swap::state_dir()`, `rope = buffer.document.buffer.snapshot()`).
     **Predicate note:** use raw `Document::dirty()`, NOT `Editor::is_dirty(id)` —
     the latter excludes scratch buffers (a quit-prompt convenience), but a scratch
     buffer with content IS unsaved work we must preserve on input-loss. Raw
     `dirty()` includes scratch-with-content and excludes clean/empty buffers.
     `write_dump` already handles a path-less (scratch) buffer via a `"scratch"`
     name fallback. This is a
     **controlled** main-loop break (not a real panic), so iterating buffers is
     safe here — unlike the panic hook, whose conservative single best-effort
     `try_lock` `dump_on_panic` stays as-is (the hook is unchanged by this effort).
     Factor this into a testable `dump_all_dirty(editor) -> usize` (count dumped).
   - `run()` returns normally so its `TerminalGuard` **drops** (restoring the
     terminal via RAII, `term.rs:65`) — the terminal is sane *before* any exit call.
   - `run()` returns `ExitReason` (its signature widens from `io::Result<()>` to
     `io::Result<ExitReason>`, or it returns the reason alongside its `Result`).
   - `main()` maps the reason: `Normal` → exit 0; `InputLost` → print a one-line
     reason to stderr (e.g. `"input reader stopped — terminal may have closed;
     recovery written"`) and `std::process::exit(<non-zero>)` *after* `run()` has
     returned (guard already dropped, terminal already restored).
   Factoring the decision into `ExitReason` keeps it unit-testable without real
   thread teardown.

This complements (does not change) the main-thread panic hook: an input-thread
panic is handled by the watchdog → main-loop path (the main thread does the
restore + dump), so the hook stays main-thread-only and we never restore the
terminal from a non-main thread (which would race concurrent terminal writes).

## Goal 2b — clipboard-death notice (Q4 → B)

The clipboard-intent drain (`clipboard.rs:34`, called from `app.rs:2095`) handles
the two request paths asymmetrically today: `clip_tx.send(ClipReq::Get{..})`
already checks `.is_err()` and synthesizes a fallback `Msg::ClipboardPaste { text:
None }`, but `clip_tx.send(ClipReq::Set(..))` is `let _ = …` — **silently
ignored, with no error detection to extend**. So this effort:
- On the **`Get`** path: at the existing `Err` branch, additionally set
  `editor.status = "clipboard unavailable"` (the `text: None` fallback is unchanged).
- On the **`Set`** path: *add* `Err` detection (replace `let _ =` with an
  `if …is_err()` arm) that sets the same `editor.status = "clipboard unavailable"`.

`Editor.status` is public (`editor.rs:293`) and in scope at the drain. ~3–4 lines;
no watchdog, no new `Msg`.

## Error-handling summary

| Failure | Response | Data loss? | Terminal | Process |
|---|---|---|---|---|
| Markdown parse panic | deduped status, empty-tree fallback (uniform; hook suppressed via guard) | none (text untouched) | intact | keeps running |
| Input thread death | terminal restored + every dirty buffer dumped (`write_dump`) + stderr reason | none (all dirty buffers persisted) | restored | exit non-zero |
| Clipboard worker death | status "clipboard unavailable" | none | intact | keeps running |

## Testing

- **Parse boundary (deterministic):** factor the guarded compute so a test can
  pass a **panicking closure** in place of the real parse and assert: `document.blocks`
  is the **empty-tree fallback** (root span `0..len`, no children), `parse_degraded`
  set, status text set; then a succeeding closure assigns the real tree, clears
  `parse_degraded`, and clears the notice. *Best-effort:* if the real
  `pulldown-cmark` panic input is cheaply reproducible, pin it as an end-to-end
  regression test (drive `rebuild` with that text, assert no crash + status set);
  not a blocker if it can't be minimized.
- **Panic-hook suppression (deterministic):** a test that, with the hook logic
  factored into a pure predicate, asserts `should_handle_panic(main, main)` returns
  `true` normally but the teardown is suppressed when the caught-panic guard is
  active (`caught_guard_active() == true`) — i.e. a main-thread `panicx::catch`
  around a panicking closure does **not** select the teardown path. (Test the
  predicate + guard state, not a real terminal teardown.)
- **Input watchdog (deterministic):** spawn a thread that returns immediately, run
  the watchdog join+send logic, assert `Msg::InputThreadDied` arrives on the
  channel. Separately, a unit test that the loop given `Msg::InputThreadDied`
  yields `ExitReason::InputLost`, and that `InputLost` drives the dump + non-zero
  exit decision (test the decision function, not real teardown).
- **All-dirty-buffer dump (deterministic):** build an editor with a dirty file
  buffer, a scratch buffer holding content, and one clean saved buffer; call
  `dump_all_dirty(editor)`, assert it returns `2` and that two `recovered-*.md`
  files were written (dirty file + scratch-with-content; the clean buffer skipped),
  each containing its buffer's text, and the scratch dump uses the `recovered-scratch-*`
  name. Reuses `write_dump` (already tested at `recovery.rs`).
- **Clipboard notice:** drop the clipboard `Receiver`, call the intent drain with
  a `Get` (and a `Set`) intent, assert `editor.status == "clipboard unavailable"`
  and the existing `text: None` fallback still fires.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact `new_len` accessor in `rebuild` for `empty_tree(new_len)` (the new
   rope's byte length) and the `Buffer::from_text` length for its fallback.
2. The precise shape of the `panicx` caught-panic guard (thread-local `Cell<bool>`
   + save/restore RAII) and the exact `term.rs` hook edit that consults
   `caught_guard_active()`; confirm the guard is established before `catch_unwind`
   so it is live when the hook runs at the panic site.
3. `run()`'s current signature/teardown (`TerminalGuard` drop at `term.rs:65`) and
   the minimal `ExitReason` threading into `main()`; confirm `dump_all_dirty` runs
   before `run()` returns and `process::exit` only after.
3a. The editor API to iterate all open buffers and read each one's rope + path for
   `dump_all_dirty` (`Editor::buffers` `editor.rs:287`; raw `Document::dirty()`
   `editor.rs:66` as the predicate — NOT `Editor::is_dirty`, so scratch-with-content
   is included; `buffer.document.buffer.snapshot()` for the rope; `document.path`) —
   so the dump touches every buffer holding unsaved work, not just the active one.
4. The exact `Msg` enum derives + the three match sites to update (manual `Debug`
   `app.rs:65–103`; main `match msg` `app.rs:1651–1756`; the pre-modal dispatch
   point so `InputThreadDied` bypasses the overlay `_ => {}` at `app.rs:1403–1442`),
   plus where `parse_degraded` state lives (App run-loop state vs `Editor`).
5. `block_tree::empty_tree(len)` — confirm `BlockTree`/`Block` public fields and the
   `BlockKind::Document` root shape so the constructor is a thin pure wrapper.
6. The clipboard drain signature at `clipboard.rs:34` — that `editor` (for
   `editor.status`) is reachable to set the notice on both the `Get` and `Set`
   `Err` branches.
