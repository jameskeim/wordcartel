# PTY smoke layer — design

**Status:** spec-review CLEAN (Codex ×6: r1 2C/3I/3M, r2 1I, r3 clean; Fable design review 9 findings folded; r4 2I/3M, r5 1I/1M, r6 clean) — ready for user review + planning
**Date:** 2026-07-03
**Effort:** pty-smoke (fills the slot the e2e-harness design deferred: "a handful of smoke
tests at most, not a harness" — `2026-07-02-wordcartel-e2e-tui-harness-design.md` Non-goals)

## Context

The in-process e2e harness (`wordcartel/src/e2e.rs`, merged `8944f95`) drives the real
`reduce → advance → render` loop against a ratatui `TestBackend` with a virtual clock. By
design it never spawns the binary, so a set of behaviors remain untested by ANY layer:

- `main()` startup: CLI parsing (`config.rs:14-30`), open-error surfacing
  (`app.rs:1904-1922`), exit codes (`main.rs:6-19`). Note: a no-file launch is an
  unnamed buffer seeded with a single newline (`Editor::new_from_text("\n", None, area)`,
  `app.rs:1900`), and `install_scratch()` (`app.rs:1927`, `editor.rs:502`) ALWAYS appends
  a `*scratch*` buffer — so even a bare launch renders `[1/2]`, never `[1/1]`.
- Real terminal lifecycle: raw mode + alt screen via `TerminalGuard` (`term.rs:33-57`,
  Drop restore at `term.rs:65-69`).
- **Panic → terminal restore → recovery dump** (`term.rs:96-120`): the panic hook
  (main-thread-only — `should_handle_panic`, `term.rs:80-85`) writes a
  `recovered-<name>-*.md` dump (`recovery.rs:52-60`), restores the terminal, then chains
  to the default hook which prints the panic. The in-process harness cannot test any of
  this — it owns no terminal.
- Real crossterm byte decoding from a PTY (vs synthetic `KeyEvent`s).
- **OSC 52 emission** as observed by a real tmux: `drain_clipboard_intents`
  (`clipboard.rs:34-60`) writes `\x1b]52;c;<base64>\x1b\\` (ST-terminated, selector `c` —
  `osc52_set`, `clipboard.rs:172`; format pinned by test `clipboard.rs:199`) directly to
  the terminal on every Copy/Cut — except when the base64 payload exceeds
  `OSC52_MAX_ENCODED`, where `osc52_set` returns `None` and the write is skipped
  (`clipboard.rs:41,172`). S5's payload is a short sentence, far below the cap. The e2e
  harness `step()` explicitly omits `drain_clipboard_intents` (documented in its
  docstring).
- The tiny-terminal guard (`render.rs:229-246`: `w < 4 || h < 2` → clamped `"..."`
  notice) under a real resize.

`tmux` gives all of this a deterministic PTY: fixed window sizes, `capture-pane` screen
scraping, `send-keys` input, and — with `set-clipboard on` — OSC 52 capture into a tmux
paste buffer that `show-buffer` can assert.

Resolved design forks (user-approved): **vendor a repo-local tmux helper** (no dependency
on personal tooling paths); **standalone runner, NOT a merge GATE** (`cargo test` + clippy
remain the gates); **add a debug-only panic trigger** so the panic-restore path is testable.

## Goals

- A vendored helper, `scripts/smoke/tmux-drive.sh`, giving check scripts a tiny stable
  vocabulary (start/start-wcartel/snap/keys/type/wait-for/wait-stable/wait-dead/stop/
  killall) on a private per-run tmux socket, hermetic from the developer's own
  tmux/desktop environment.
- Eight seed smoke checks (S1–S8 below) covering exactly the real-binary surface the
  in-process harness cannot: startup/exit, open-error handling, real-terminal save
  round-trip, dirty-quit modal on a real screen, OSC 52 → tmux buffer, tiny-terminal
  guard, panic → restore → recovery dump, and hard-kill → swap → recovery.
- A `#[cfg(debug_assertions)]` panic trigger (`WCARTEL_SMOKE_PANIC`) — compiled out of
  release builds; the only Rust change in this effort.
- `scripts/smoke/run.sh`: runs all checks, per-check PASS/FAIL, nonzero exit on any
  failure, graceful skip when `tmux` is absent.
- CLAUDE.md documentation: running the suite and quoting its one-line summary in the
  pre-merge report is MANDATORY for every effort; a red result is ADVISORY (does not
  block merge, must be surfaced to the human as a finding). This decouples *run* from
  *pass*: the not-a-GATE fork holds (red never blocks), while guaranteeing the suite
  executes — the only way a stability record accumulates.

## Non-goals

- **Not a GATE.** `cargo test` green + workspace clippy clean remain the only merge
  gates; a red smoke result never blocks a merge (it is surfaced as an advisory
  finding). Running the suite, however, IS a mandatory pre-merge step (see Goals).
  Promotion to a gate later is a CLAUDE.md edit, contingent on the observed stability
  record.
- **Not a journey harness.** Editing journeys, invariants, and regression pins stay in
  `wordcartel/src/e2e.rs` (its 7 seed journeys are listed there); the smoke layer does
  not duplicate them. S3/S4 overlap e2e's save/quit journeys at the logic level only —
  their purpose here is validating the full real-binary stack, and they are the cheapest
  checks that do so.
- **No release-binary behavior change.** The panic trigger is `debug_assertions`-gated.
- **No CI setup.** The repo has none; adding it is out of scope.
- **No general TUI-driving library.** The helper is ~60 lines serving these checks; the
  user's interactive `tui-interact` skill remains separate and personal.

## Component 1 — vendored helper: `scripts/smoke/tmux-drive.sh`

POSIX sh. Commands (first arg), operating on the per-run socket exported by the runner
(`tmux -L "$SMOKE_SOCKET"`, e.g. `wcartel-smoke-$$` — see Per-run socket below):

| Command | Behavior |
|---|---|
| `start <session> <cmd> [--cols N] [--rows N]` | New detached session (default 120×40) with **`remain-on-exit on`**, running `<cmd>` directly as the pane command — NO interactive shell, no prompt scraping, no command echo |
| `start-wcartel <session> <args…>` | `start` + the full hermetic launch env (below) + **blocks on the notice barrier** (`wait-for 'system clipboard unavailable'`) before returning — the only way checks launch the app |
| `snap <session>` | `capture-pane -p` — plain-text visible screen |
| `keys <session> <key>...` | `send-keys` with named keys (`Enter`, `Escape`, `C-q`, …) |
| `type <session> <text>` | `send-keys -l` — literal text, no key-name interpretation |
| `wait-for <session> <ere> [timeout]` | Poll `snap` (0.2s interval) until the ERE matches; default 10s; on timeout print the final screen and exit 1 |
| `wait-stable <session> [timeout]` | Poll until two consecutive captures are identical — a settling aid ONLY, never an assertion primitive (see Robustness) |
| `wait-dead <session> [timeout]` | Poll `#{pane_dead}` until 1; prints `#{pane_dead_status}` |
| `stop <session>` | `kill-session` (idempotent) |
| `killall` | `kill-server` on THIS RUN's socket (idempotent) |

**Exit-code assertions use tmux, not a shell:** because the pane runs the app directly
under `remain-on-exit on`, process exit leaves a dead pane whose final screen (including
post-restore stderr, e.g. S7's panic message) stays exactly as the app left it — nothing
scrolls it away, and `snap` still reads it. Checks assert exit via
`wait-dead` + `#{pane_dead_status}` (tmux ≥2.2; stable) instead of scraping a shell
prompt — which would otherwise depend on the developer's login shell, prompt theme, and
rc-file noise (the flakiest surface a PTY suite can have).

**Per-run socket:** the runner generates a unique socket name (`wcartel-smoke-$$`) and
exports it to checks; `killall` only ever kills this run's server, so concurrent runs
(human + review agent — the stated adoption model) cannot destroy each other, and S5's
server-global paste buffer cannot be corrupted by an interleaved run. A separate
`scripts/smoke/clean-stale.sh` sweeps abandoned `wcartel-smoke-*` sockets by glob.

**Hermeticity rules (the reason this isn't just tui.sh copied):**

1. The smoke server starts with **`-f /dev/null`** and then explicitly
   `set -s set-clipboard on`. Rationale: tmux 3.x auto-loads `~/.config/tmux/tmux.conf`;
   captures and clipboard behavior must not depend on the developer's config. And the
   default `set-clipboard external` does not store inner-app OSC 52 into tmux paste
   buffers — `on` does, which S5's `show-buffer` assertion requires.
2. Every `start-wcartel` launches the app under
   **`env -u TMUX -u DISPLAY -u WAYLAND_DISPLAY XDG_STATE_HOME=<per-run tempdir>`**:
   - `-u TMUX` — prevents leakage to any outer server (an observed hazard).
   - `-u DISPLAY -u WAYLAND_DISPLAY` — forces `arboard::Clipboard::new()` to fail
     (`clipboard.rs:107-108`), so `Msg::ClipboardAvailability(false)` and the
     once-only headless notice (`app.rs:808-813`) are deterministic on every machine,
     desktop or headless.
   - `XDG_STATE_HOME=<tempdir>` — recovery dumps (`recovery.rs`, via `swap::state_dir()`)
     land in a per-run temp dir the checks can inspect and delete; the developer's real
     state dir is never touched.
3. All wcartel launches pass **`--no-config`** (`config.rs:14-30`) so user config —
   including a WordStar-style keymap that rebinds `ctrl-c` away from `copy`
   (`keymap.rs:336`) — cannot change the keys the checks send. Checks assert against the
   DEFAULT keymap (`keymap.rs:230-238`: `ctrl-c` copy, `ctrl-s` save, `ctrl-q` quit).

## Component 2 — the seed checks (S1–S8)

Each check is its own script `scripts/smoke/checks/s<N>-<name>.sh`, sourcing the helper,
creating/destroying its own session, and exiting 0/1. Screen assertions use the exact
strings the explore pass pinned:

- **S1 — startup & clean quit.** `start-wcartel` with no file (`--no-config`):
  `wait-for '\*untitled\*'` (unnamed buffer name, `workspace.rs:7-19`); status head
  matches `\[1/2\]` (`render.rs:178` — the launch buffer plus the always-installed
  `*scratch*`, `app.rs:1927`/`editor.rs:502`). `C-q` on the clean buffers → no modal
  (`commands.rs:526-538`: not dirty → immediate quit) → `wait-dead` reports
  `pane_dead_status` = 0 (`main.rs:6-19`).
- **S2 — open-error handling.** The startup status strings (`new file`, `…: permission
  denied`) are set synchronously at launch (`app.rs:1909,1921`) but are asynchronously
  OVERWRITTEN by the headless clipboard notice (see the race note under Robustness), so
  checks assert the **durable observables** — the status-line head — not the transient
  message: (a) nonexistent path → head shows `[1/2] <basename> [PREVIEW]` (opened-as-new
  semantics: `editor.rs:224-230` folds `NotFound` into an empty buffer carrying the
  path); app alive. (b) a `chmod 000` temp file → the open fails (`OpenError::
  Permission`, `app.rs:1915-1922`) and the app continues on the unnamed LAUNCH buffer —
  head shows `[1/2] *untitled*` (the fallback is the launch buffer, `app.rs:1900`; the
  `*scratch*` buffer is a separate second buffer, `editor.rs:499-506` — NOT the active
  fallback); app alive. Root guard: `chmod 000` is a no-op for uid 0 (containers) — the
  check pre-flights that the file is actually unreadable (`cat` fails) and SKIPS this
  sub-assertion otherwise.
- **S3 — save round-trip.** Open a temp path; **notice barrier first** (see Robustness —
  `wait-for 'system clipboard unavailable'` so the one-shot async status writer has
  provably fired); then `type` a sentence → dirty marker `\*<name>` appears in the head;
  `C-s` → `wait-for 'Saved'` (`save.rs:99` — race-free only AFTER the barrier); marker
  gone; file on disk *contains* the typed sentence (exact EOL policy is the app's
  concern, not the check's).
- **S4 — dirty-quit modal.** Type into an unnamed buffer, `C-q` →
  `wait-for 'unsaved: \[A\]ll save · \[R\]eview each · \[C\]ancel'`
  (`prompt.rs:64-71`); send `c` → cancel. "Modal gone" is asserted POSITIVELY, not by
  absence: type one more character and `wait-for` it on screen (a character can only
  land in the buffer if the modal released input) — never a bare stable-then-absent
  check. `C-q` again → `r` → `wait-for ': \[S\]ave · \[D\]iscard · \[C\]ancel'`
  (`prompt.rs:75-83`) → `d` → app exits (last dirty buffer discarded) → `wait-dead`,
  status 0.
- **S5 — clipboard: headless notice + OSC 52 → tmux buffer.** At startup,
  `wait-for 'system clipboard unavailable'` (once-only notice, `app.rs:810` — guaranteed
  by the `-u DISPLAY -u WAYLAND_DISPLAY` launch env). Type a known sentence, select it
  with **shift-arrow extension** (default-layer bindings exist at `keymap.rs:261`; sent
  as tmux `S-Left`/`S-Right` key names — a non-empty selection is required because Copy
  no-ops on empty, `commands.rs:403-418`), `C-c` → `wait-for 'Copied'`
  (`commands.rs:416`) AND `tmux -L <run-socket> show-buffer` equals the selected text
  (OSC 52 captured because the smoke server sets `set-clipboard on`).
- **S6 — tiny-terminal guard.** Start at `--cols 60 --rows 15` → normal render (status
  line present, typed text visible). `tmux resize-window` to 3×1 → `wait-for` the `...`
  guard (`render.rs:229-246`), no crash (pane NOT dead). Resize back to
  60×15 → content restored (typed text visible again — a real-terminal cousin of the
  Resize-blank regression class).
- **S7 — panic → restore → recovery dump.** Launch with `WCARTEL_SMOKE_PANIC=1` (debug
  binary), type a sentence (buffer dirty — ordinary keys pass through the trigger), then
  send **F12** → the trigger panics on the main thread. Assert: (a) `wait-dead` — the
  pane is dead with nonzero status, and because `remain-on-exit` holds the dead pane's
  final screen, `snap` shows the alt screen was LEFT (panic output, not a wrecked editor
  frame — nothing scrolls it away); (b) the panic message text
  (`WCARTEL_SMOKE_PANIC: deliberate smoke-test panic`) is visible on the dead pane's
  screen (the chained default hook prints to stderr AFTER restore, `term.rs:117`);
  (c) exactly one `recovered-*.md` file exists under `<tempdir>/wordcartel/` and
  contains the typed sentence (`recovery.rs:37-60` dump path via `swap::state_dir()`).
- **S8 — hard-kill → swap → recovery offer (the charter's data-loss check).** The
  binary has NO signal handling; its real defense for "terminal closed with unsaved
  edits" is the idle swap writer (`swap.rs`: `T_IDLE_MS = 2_000`) plus the open-time
  recovery prompt (`app.rs:1969-1982`, `Prompt::swap_recovery()`). Only a PTY layer can
  exercise it end-to-end: `start-wcartel` on a temp path, type a sentence, **poll the
  temp `XDG_STATE_HOME` for the swap file** (a filesystem wait-for, NOT a blind 2s
  sleep) → `kill-session` (destroys the session and its pty; the source has no signal
  handling, so the process dies with no cleanup — the swap file survives) → fresh
  `start-wcartel` on the SAME path → `wait-for` the swap-recovery prompt on a real
  screen. Literal prompt text (`prompt.rs:100`):
  `Recovery file found: [R]ecover · [D]iscard · [O]pen original`; the check's wait-for
  ERE escapes the brackets:
  `Recovery file found: \[R\]ecover · \[D\]iscard · \[O\]pen original`. Send `r`
  (prompt input is lowercased, `prompt.rs:43`; Recover bound to 'r', `prompt.rs:102`) →
  assert the recovered buffer shows the typed sentence. Covers the most common
  real-world data-loss event; the existing in-process test (`app.rs:2742`) covers the
  reducer only.

## Component 3 — the panic trigger (only Rust change)

A `debug_assertions`-gated check in the main loop's key-event dispatch path (exact
insertion point pinned at plan time against `app.rs` — it must be on the **main thread**,
inside the same iteration that calls `reduce` for a `Key` message, because the panic hook
ignores non-main threads, `term.rs:80-85`):

```rust
// Inserted BEFORE the modal/minibuffer branches. No pre-modal KeyEvent binding
// exists there (the modal arm introduces `key` at app.rs:1430; minibuffer uses `k`
// at app.rs:1479), so the trigger performs its OWN destructure of the incoming
// event. `ev` is illustrative — the plan pins the real event variable name + anchor.
#[cfg(debug_assertions)]
if let crossterm::event::Event::Key(key) = &ev {
    if key.kind == crossterm::event::KeyEventKind::Press
        && key.code == crossterm::event::KeyCode::F(12)
        && std::env::var_os("WCARTEL_SMOKE_PANIC").is_some()
    {
        panic!("WCARTEL_SMOKE_PANIC: deliberate smoke-test panic");
    }
}
```

(`app.rs` imports only `crossterm::event::Event` at `app.rs:6`; existing code uses the
fully qualified `crossterm::event::KeyCode` form, e.g. `app.rs:1683` — the snippet
matches that convention rather than assuming an import.)

Semantics: fires when an **F12** key event is dispatched while the env var is set (any
value); all other keys behave normally, so S7 can type text (dirtying the buffer) before
triggering. **F12 is confirmed unbound in the default keymap** (the CUA defaults,
`keymap.rs:222-318`, bind no `f12`). Two placement requirements: (a) the check sits
BEFORE modal/minibuffer key interception (`app.rs:1424`/`1475`) so it fires regardless
of app state; (b) it gates on `KeyEventKind::Press` (matching the app's existing kind
filtering) so key-repeat/release under enhanced keyboard protocols cannot double-fire.
Release builds compile the check out; the var is otherwise inert. House-style note: the key-code comparison short-circuits before the env
read, and only in debug builds — no release-path cost. The check must be clippy-clean
under the workspace `all = "deny"` gate.

## Component 4 — runner and docs

- `scripts/smoke/run.sh`: environment pre-flight — `command -v tmux` AND tmux ≥ 3.0
  (`resize-window` needs ≥2.9; inner-OSC-52-to-buffer semantics are stable in 3.x);
  either failing prints a one-line skip notice and exits 0 (skip is exit 0 by design —
  the layer is advisory). Generates the per-run socket name and temp dirs; resolves the
  binary ONCE as `WCARTEL_BIN=${CARGO_TARGET_DIR:-target}/debug/wcartel` and exports it
  (a bare `target/debug/` path breaks under `CARGO_TARGET_DIR`); `cargo build` (debug —
  S7 requires debug assertions; also keeps compile output out of session captures); run
  S1–S8 sequentially; per-check `PASS s<N>` / `FAIL s<N>` lines; a one-line summary
  (`smoke: 8/8 PASS` / `smoke: FAIL s5 — advisory`) appended to a history file
  (`scripts/smoke/.history`, gitignored) so the stability record accumulates; exit 1 if
  any failed. Rerun-safe.
- CLAUDE.md: a short subsection under the Rust-conventions/GATE area — **running
  `scripts/smoke/run.sh` and quoting its one-line summary in the pre-merge report is
  mandatory; a red result is advisory** (never blocks a merge; must be listed as a
  finding for the human). Amend the hardening-campaign "New candidate: an e2e/TUI
  harness … untouched frontier" note to record that both layers now exist (in-process
  journeys in `e2e.rs`; PTY smoke in `scripts/smoke/`).

## Robustness rules (bind all checks)

- No bare `sleep`s for readiness — `wait-for` on a known string, `wait-stable` after
  input; 10s ceilings.
- `wait-stable` is a settling aid, never an assertion primitive: two identical captures
  0.2s apart can coincide mid-transition. Positive assertions always use `wait-for` on a
  pinned string; negative assertions ("modal gone") are re-expressed positively where
  possible (S4 types a character and waits for it to land).
- Anchor/choose assertion strings so they cannot collide with OTHER on-screen text —
  the status head contains the file basename, and a typed sentence appears in the
  buffer as well as (potentially) a status message. (Command echo is a non-issue: the
  pane-dead architecture runs no interactive shell, so nothing echoes.)
- Every check `trap`s cleanup: `stop` its session, remove its temp files/dirs.
- Fixed window sizes; every capture is deterministic given the app state.
- Status-line strings are stable ONCE THE APP IS QUIESCENT: a status message persists
  until the next resolved key dispatch clears it (`app.rs:1696,1707`) — no timer races a
  capture. **But startup is not quiescent:** the clipboard worker (spawned at
  `app.rs:2025`) delivers `ClipboardAvailability(false)` asynchronously, and
  `apply_clipboard_availability` (`app.rs:808-813`, dispatched at `app.rs:1784`)
  OVERWRITES whatever status is showing at a nondeterministic moment — possibly after a
  check has already typed or saved (key dispatch clears only the status present AT
  dispatch time, `app.rs:1707`; it cannot consume a message that hasn't arrived yet).
  Two rules follow:
  1. **No check may assert a startup-time status message** (`new file`, open errors) —
     assert durable head strings instead (S2).
  2. **Notice barrier:** any check that asserts a dispatch-set status (`Saved` in S3,
     `Copied` in S5) MUST first `wait-for 'system clipboard unavailable'` after startup.
     The forced-headless env guarantees the notice fires exactly once
     (`clipboard_notice_shown`, `app.rs:811`); after the barrier, no async status writer
     remains and dispatch-set statuses are race-free.

## Verification of this effort

1. `scripts/smoke/run.sh` passes end-to-end on this machine (all 8 checks PASS).
1b. **Burn-in:** ≥20 consecutive full-suite runs with zero failures before the effort is
   declared done — one green run proves plumbing, not stability, and stability is the
   suite's entire charter (never cry wolf). The history file records the runs.
2. Sabotage test during development: each check observed to FAIL against a deliberately
   wrong assertion (proves the polling/assert plumbing can fail, not just pass).
3. `cargo build --release` + run with `WCARTEL_SMOKE_PANIC=1` → var ignored, no panic
   (trigger compiled out).
4. Existing GATEs unaffected: `cargo test` green, workspace clippy clean (the trigger
   must be clippy-clean under `all = "deny"`).
5. Developer's environment untouched: no sessions on any non-smoke tmux socket, real
   `~/.local/state/wordcartel` unmodified (checks use the temp `XDG_STATE_HOME`).

## Open items (pin during planning)

- Exact insertion point (file:line) for the panic trigger in `app.rs`'s key-dispatch
  arm (honoring the two placement requirements in Component 3).
- Whether `run.sh` builds with `--quiet` or captures cargo output to a log file.
(Shell-prompt detection was ELIMINATED by the remain-on-exit/pane-dead architecture —
no interactive shell exists in any session.)

(Resolved during spec review: S5 selection = default-layer shift-arrows, `keymap.rs:261`;
F12 confirmed unbound in the default keymap, `keymap.rs:222-318`.)
