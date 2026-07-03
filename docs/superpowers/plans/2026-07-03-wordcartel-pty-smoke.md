# PTY Smoke Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give wordcartel the real-binary smoke layer the in-process e2e harness cannot provide: a vendored tmux helper, eight seed checks (startup/exit, open errors, save round-trip, dirty-quit modal, OSC 52 → tmux buffer, tiny-terminal guard, panic → restore → recovery dump, hard-kill → swap → recovery), one debug-only panic trigger, an advisory runner, and the mandatory-run/advisory-pass CLAUDE.md policy.

**Architecture:** All checks drive the actual `wcartel` debug binary inside a private per-run tmux server (`-L wcartel-smoke-$$`, `-f /dev/null`). Each pane runs the app DIRECTLY (no interactive shell) under `remain-on-exit on`, so process exit leaves a dead pane whose frozen screen and `#{pane_dead_status}` are the assertion surface — no prompt scraping, ever. Every launch is hermetic (`env -u TMUX -u DISPLAY -u WAYLAND_DISPLAY XDG_STATE_HOME=<per-check tempdir>`, `--no-config`) and, except S8's relaunch, blocks on the once-only headless-clipboard notice barrier before any assertion. The only Rust change is a `#[cfg(debug_assertions)]` F12+env-var panic trigger at the very top of `reduce`.

**Tech Stack:** POSIX sh, tmux ≥3.0, cargo/rustc (one debug-only trigger), no new crates.

**Spec (source of truth):** `docs/superpowers/specs/2026-07-03-wordcartel-pty-smoke-design.md`
**Branch:** `effort-pty-smoke`

## Global Constraints

Verbatim from the spec — every task's requirements implicitly include this section:

- **Not a GATE / mandatory-run + advisory-pass.** `cargo test` green + workspace clippy clean remain the only merge gates; a red smoke result never blocks a merge (it is surfaced as an advisory finding). Running the suite and quoting its one-line summary in the pre-merge report IS a mandatory pre-merge step.
- **Per-run socket** `wcartel-smoke-$$` — the runner generates and exports it; `killall` only ever kills THIS run's server; concurrent runs (human + review agent) cannot destroy each other; S5's server-global paste buffer cannot be corrupted by an interleaved run.
- **Pane-dead architecture** — no interactive shell in any session: the pane runs the app directly under `remain-on-exit on`; exit codes come from `wait-dead` + `#{pane_dead_status}`, never a scraped shell prompt.
- **`start-wcartel` hermetic env** — `env -u TMUX -u DISPLAY -u WAYLAND_DISPLAY XDG_STATE_HOME=<per-check tempdir>` plus `--no-config` on every launch, then the **notice barrier** (`wait-for 'system clipboard unavailable'`) before returning. It is the only way checks launch the app.
- **Server config** — `-f /dev/null` on every tmux invocation, then explicit `set -s set-clipboard on` (default `external` does not store inner-app OSC 52 into tmux paste buffers; `on` does — S5 requires it).
- **Binary resolution** — `WCARTEL_BIN=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel`, resolved once and exported (a bare `target/debug/` path breaks under `CARGO_TARGET_DIR`; `$PWD/target` is the equivalent form in Task 2's steps, since all plan commands run from the repo root).
- **Trigger** — `debug_assertions`-only; compiled out of release builds; must be clippy-clean under the workspace `[workspace.lints.clippy] all = "deny"` gate.
- **House style** — wordcartel is hand-formatted: NEVER run `cargo fmt`; match neighbors by hand; `—` (em-dash) in prose comments, never `--`; no emoji in code.
- **Commit trailers** — every commit message in this plan ends with, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
  ```
- **Burn-in** — ≥20 consecutive full-suite runs with zero failures before the effort is declared done; the history file records the runs.
- **Never touch** the user's tmux sockets or the real `~/.local/state/wordcartel` state dir.

---

## Pinned Resolutions (plan-time, verified against real source)

The spec left four items open for planning. All four are now pinned, plus one latent conflict found while pinning:

1. **Panic-trigger anchor + real event variable.** The insertion point is the very top of `pub fn reduce` in `wordcartel/src/app.rs` — the body opens at `app.rs:1099` (`) -> bool {`); the trigger is inserted immediately after it, BEFORE the `// pending_mark intercepts…` comment at `app.rs:1100`. This is strictly stronger than the spec's "before the modal/minibuffer branches (1424/1475)" requirement: it also precedes the pending-mark (1102), menu (1120), palette (1170), theme-picker (~1246), and file-browser (1334) interception branches, so it fires regardless of app state. `reduce` is invoked on the **main thread** by the real run loop (`app.rs:2181`: `let keep = reduce(msg, …)`), satisfying the panic-hook main-thread requirement (`term.rs:80-85`). The real event variable is the function parameter **`msg: Msg`** — no pre-modal `KeyEvent` binding exists there, so the trigger destructures `Msg::Input(Event::Key(key))` itself (`Event` is already imported at `app.rs:6`). The final code block is in Task 2. The nested `if let` + inner-`if` shape mirrors the existing clippy-clean pattern at `app.rs:1103-1104`.
2. **Swap-file naming/location** (`wordcartel/src/swap.rs:135-149`). Named docs: `<XDG_STATE_HOME>/wordcartel/<sanitized-basename>-<16-hex fnv1a64 of the canonicalized realpath>.swp` (e.g. `s8-doc.md-a1b2c3d4e5f60718.swp`); scratch: `scratch-<pid>.swp`. S8's filesystem poll therefore globs `"$SMOKE_STATE_HOME/wordcartel/s8-doc.md-"*.swp`. (S7's recovery dump is `recovered-<name>-<pid>-<seq>.md` in the same dir, `recovery.rs:26`; the launch buffer is path-less so its dump name is `recovered-scratch-<pid>-0.md` — the check globs `recovered-*.md`.)
3. **run.sh cargo-build output handling.** Redirect `cargo build -p wordcartel` stdout+stderr to `"$RUN_DIR/build.log"` (the run's temp dir); on build failure, `cat` the log to stderr and exit 1. Keeps compile noise out of the run transcript and session captures while losing nothing on failure.
4. **S6's 3×1 resize is achievable.** Verified empirically on this machine (tmux 3.6b): `tmux resize-window -x 3 -y 1` on a detached 120×40 session reports `#{window_width}x#{window_height}` = `3x1` — no clamping. Target stays **3×1**, which trips both halves of the `w < 4 || h < 2` guard (`render.rs:237`). Pre-flight already requires tmux ≥3.0 (`resize-window` exists since 2.9).
5. **S8 barrier conflict (found at plan time).** An active modal prompt REPLACES the status row (`render.rs:631-633` renders `prompt.message` instead of the status text), and S8's relaunch raises the swap-recovery prompt at open (`app.rs:1969-1982`) — so the clipboard notice cannot appear on screen until the prompt resolves, and a mandatory barrier would deadlock the check into a timeout. Resolution: `start-wcartel` accepts a `--no-barrier` flag used by exactly one launch in the whole suite — S8's relaunch — whose immediate `wait-for` on the literal recovery-prompt text is that launch's own (stronger) barrier. All other launches keep the mandatory barrier.

Also pinned against source (assertion strings the checks use, all verified):
`"system clipboard unavailable"` (`app.rs:810`); `"Saved"` (`save.rs:99`); `"Copied"` (`commands.rs:416`); `"{n} buffer(s) unsaved: [A]ll save · [R]eview each · [C]ancel"` (`prompt.rs:66`); `"{name}: [S]ave · [D]iscard · [C]ancel"` (`prompt.rs:78`); `"Recovery file found: [R]ecover · [D]iscard · [O]pen original"` (`prompt.rs:100`); status head `[{idx}/{count}] {name}` + mode `[PREVIEW]` (`render.rs:174-186`); dirty prefix `*` on the display name (`workspace.rs:18`); `*untitled*` (`workspace.rs:15`); tiny-guard notice `"..."` (`render.rs:239`); default keymap `ctrl-c` copy / `ctrl-s` save / `ctrl-q` quit / `shift-left` select (`keymap.rs:230-265`); `word_count` defaults to `false` (`config.rs:87`) so the status row has no right-flush segment and the barrier string fits a 60-col window (head `[1/2] *untitled* [PREVIEW] ` = 27 chars + 28-char phrase = col 55 < 60).

---

## File Structure

```
scripts/smoke/
├── tmux-drive.sh                 — vendored helper: t/boot/start/start_wcartel/snap/keys/
│                                   type_text/wait_for/wait_stable/wait_dead/stop/killall;
│                                   CLI dispatch when executed directly (Task 1)
├── clean-stale.sh                — sweep abandoned wcartel-smoke-* sockets by glob (Task 1)
├── run.sh                        — pre-flight, per-run socket + tempdir, debug build,
│                                   S1–S8, PASS/FAIL lines, summary, .history (Task 11)
├── .history                      — appended one-line summaries; gitignored (Task 11)
└── checks/
    ├── s1-startup-quit.sh        — Task 3
    ├── s2-open-errors.sh         — Task 4
    ├── s3-save-roundtrip.sh      — Task 5
    ├── s4-dirty-quit-modal.sh    — Task 6
    ├── s5-clipboard-osc52.sh     — Task 7
    ├── s6-tiny-terminal.sh       — Task 8
    ├── s7-panic-recovery.sh      — Task 9
    └── s8-kill-swap-recovery.sh  — Task 10
wordcartel/src/app.rs             — panic trigger at top of reduce; only Rust change (Task 2)
CLAUDE.md                         — mandatory-run/advisory-pass subsection + frontier amend (Task 12)
.gitignore                        — + scripts/smoke/.history (Task 11)
```

Environment contract between the pieces (all exported):

| Variable | Set by | Meaning |
|---|---|---|
| `SMOKE_SOCKET` | run.sh (`wcartel-smoke-$$`); a standalone check defaults it and then owns/kills the server | per-run private tmux socket name |
| `SMOKE_TMPDIR` | run.sh (per-run `mktemp -d`); standalone checks fall back to `${TMPDIR:-/tmp}` | parent for per-check work dirs |
| `SMOKE_STATE_HOME` | each check (subdir of its own work dir) | the launch's `XDG_STATE_HOME` |
| `WCARTEL_BIN` | run.sh (`${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel`); standalone checks default identically | binary under test |
| `WCARTEL_SMOKE_PANIC` | S7 only | forwarded into the launch env by `start_wcartel` when set |

If `$WCARTEL_BIN` is missing at any step (fresh worktree): `cargo build -p wordcartel` and retry — the helper's "build the debug binary first" error means exactly this.

All commands below run from the repo root `/home/jkeim/projects/groundwords` on branch `effort-pty-smoke`.

---

### Task 1: Vendored tmux helper + stale-socket sweeper

**Files:**
- Create: `scripts/smoke/tmux-drive.sh`
- Create: `scripts/smoke/clean-stale.sh`

**Interfaces:**
- Consumes: nothing (first task).
- Produces (used by every later task): sourced functions `start <session> [--cols N] [--rows N] -- <cmd> [args…]`, `start_wcartel <session> [--cols N] [--rows N] [--no-barrier] [wcartel-args…]`, `snap <session>`, `keys <session> <key>…`, `type_text <session> <text>`, `wait_for <session> <ere> [timeout-s]` (exit 1 + final-screen dump on timeout), `wait_stable <session> [timeout-s]`, `wait_dead <session> [timeout-s]` (prints `#{pane_dead_status}` to stdout), `stop <session>`, `killall`, and the raw wrapper `t` (= `tmux -L "$SMOKE_SOCKET" -f /dev/null`). CLI dispatch maps `start-wcartel`→`start_wcartel`, `type`→`type_text`, `wait-for`→`wait_for`, `wait-stable`→`wait_stable`, `wait-dead`→`wait_dead`. Requires env: `SMOKE_SOCKET` always; `WCARTEL_BIN` + `SMOKE_STATE_HOME` for `start_wcartel`.

- [ ] **Step 1: Write `scripts/smoke/tmux-drive.sh`**

```sh
#!/bin/sh
# tmux-drive.sh — vendored tmux driver for the wordcartel PTY smoke checks.
#
# POSIX sh. Two usage modes:
#   sourced (the checks):  . "$CHECK_DIR/../tmux-drive.sh"   → call the functions
#   CLI (dev convenience): scripts/smoke/tmux-drive.sh <command> [args...]
# The sourced function for the `type` command is named type_text (a function
# named `type` would shadow the shell builtin).
#
# Every command operates on the private per-run server named by $SMOKE_SOCKET
# (exported by run.sh, or defaulted by a standalone check) — NEVER the user's
# own tmux sockets. The server is created config-free (-f /dev/null) with:
#   set-clipboard on   — inner-app OSC 52 stored into tmux paste buffers (S5)
#   remain-on-exit on  — dead panes keep their final screen + exit status
#                        (the pane-dead architecture: no shell, no prompt
#                        scraping; exit codes come from #{pane_dead_status})
#   exit-empty off     — killing the only check session (S8's hard kill) must
#                        not take the server down mid-check

: "${SMOKE_SOCKET:?SMOKE_SOCKET must be set (per-run socket, e.g. wcartel-smoke-\$\$)}"

# Every invocation carries -f /dev/null so whichever call creates the server
# does so config-free (tmux 3.x otherwise auto-loads ~/.config/tmux/tmux.conf).
t() { tmux -L "$SMOKE_SOCKET" -f /dev/null "$@"; }

# Boot the server (idempotent): a keeper session pins it alive and lets the
# global options land BEFORE any check pane spawns — otherwise an instantly
# exiting pane command could die before remain-on-exit takes effect.
boot() {
    if ! t has-session 2>/dev/null; then
        t new-session -d -s __keeper -x 4 -y 2 -- sleep 3600
        t set -g remain-on-exit on
        t set -s set-clipboard on
        t set -s exit-empty off
    fi
}

# start <session> [--cols N] [--rows N] -- <cmd> [args...]
# New detached session (default 120x40) running the argv DIRECTLY as the pane
# command — tmux exec()s multiple command arguments without a shell (a single
# string would be run via /bin/sh -c, tmux(1)); no shell, no prompt scraping,
# no command echo.
start() {
    _s=$1; shift
    _cols=120; _rows=40
    while [ $# -gt 0 ]; do
        case $1 in
            --cols) _cols=$2; shift 2 ;;
            --rows) _rows=$2; shift 2 ;;
            --) shift; break ;;
            *) echo "start: unknown option: $1 (command goes after --)" >&2; exit 2 ;;
        esac
    done
    [ $# -gt 0 ] || { echo "start: no command after --" >&2; exit 2; }
    boot
    t new-session -d -s "$_s" -x "$_cols" -y "$_rows" -- "$@"
}

# start-wcartel <session> [--cols N] [--rows N] [--no-barrier] [wcartel args...]
# The ONLY way checks launch the app. Hermetic launch env:
#   -u TMUX                        — no leakage to any outer tmux server
#   -u DISPLAY -u WAYLAND_DISPLAY  — arboard init fails deterministically, so
#                                    the once-only headless notice always fires
#   XDG_STATE_HOME=$SMOKE_STATE_HOME — swap/recovery land in a per-check tempdir
# plus --no-config (checks assert against the DEFAULT keymap). Forwards
# WCARTEL_SMOKE_PANIC when the caller has it set (S7). Blocks on the notice
# barrier before returning; after it, no async status writer remains and
# dispatch-set statuses (Saved/Copied) are race-free. --no-barrier exists for
# exactly ONE launch in the suite — S8's relaunch, where the swap-recovery
# modal replaces the status row (render.rs status branch) so the notice cannot
# appear until the prompt resolves; that check's wait-for on the prompt text
# is its own, stronger barrier.
start_wcartel() {
    _sess=$1; shift
    : "${WCARTEL_BIN:?WCARTEL_BIN must be set}"
    : "${SMOKE_STATE_HOME:?SMOKE_STATE_HOME must be set}"
    [ -x "$WCARTEL_BIN" ] || { echo "start-wcartel: $WCARTEL_BIN missing — build the debug binary first" >&2; exit 1; }
    _wcols=120; _wrows=40; _barrier=yes
    while [ $# -gt 0 ]; do
        case $1 in
            --cols) _wcols=$2; shift 2 ;;
            --rows) _wrows=$2; shift 2 ;;
            --no-barrier) _barrier=no; shift ;;
            *) break ;;   # remaining args are wcartel args (paths)
        esac
    done
    # Forward WCARTEL_SMOKE_PANIC only when the caller has it SET (S7): an
    # unconditional empty assignment would still read as Some("") through
    # env::var_os on the Rust side and mis-fire the trigger. Explicit branch,
    # fully quoted — no ${VAR+word} field-splitting ambiguity across shells.
    if [ -n "${WCARTEL_SMOKE_PANIC+x}" ]; then
        start "$_sess" --cols "$_wcols" --rows "$_wrows" -- \
            env -u TMUX -u DISPLAY -u WAYLAND_DISPLAY \
                "XDG_STATE_HOME=$SMOKE_STATE_HOME" \
                "WCARTEL_SMOKE_PANIC=$WCARTEL_SMOKE_PANIC" \
                "$WCARTEL_BIN" --no-config "$@"
    else
        start "$_sess" --cols "$_wcols" --rows "$_wrows" -- \
            env -u TMUX -u DISPLAY -u WAYLAND_DISPLAY \
                "XDG_STATE_HOME=$SMOKE_STATE_HOME" \
                "$WCARTEL_BIN" --no-config "$@"
    fi
    if [ "$_barrier" = yes ]; then
        wait_for "$_sess" 'system clipboard unavailable'
    fi
}

# snap <session> — plain-text visible screen (works on dead panes too:
# remain-on-exit freezes the final screen).
snap() { t capture-pane -p -t "$1"; }

# keys <session> <key>... — named keys (Enter, Escape, C-q, S-Left, F12, ...).
keys() { _k=$1; shift; t send-keys -t "$_k" "$@"; }

# type <session> <text> — literal text, no key-name interpretation.
type_text() { t send-keys -t "$1" -l "$2"; }

# wait-for <session> <ere> [timeout-s] — poll snap (0.2s) until the ERE
# matches; default 10s; on timeout print the final screen and exit 1.
wait_for() {
    _ws=$1; _ere=$2; _wt=${3:-10}
    _i=0; _wmax=$((_wt * 5))
    while [ "$_i" -lt "$_wmax" ]; do
        if snap "$_ws" | grep -qE "$_ere"; then return 0; fi
        sleep 0.2
        _i=$((_i + 1))
    done
    echo "wait-for: timeout (${_wt}s) on session '$_ws' waiting for ERE: $_ere" >&2
    echo "---- final screen ----" >&2
    snap "$_ws" >&2 || true
    echo "----------------------" >&2
    exit 1
}

# wait-stable <session> [timeout-s] — poll until two consecutive captures are
# identical. A settling aid ONLY, never an assertion primitive (two identical
# captures 0.2s apart can coincide mid-transition).
wait_stable() {
    _ss=$1; _st=${2:-10}
    _i=0; _smax=$((_st * 5))
    _prev=$(snap "$_ss")
    while [ "$_i" -lt "$_smax" ]; do
        sleep 0.2
        _cur=$(snap "$_ss")
        if [ "$_cur" = "$_prev" ]; then return 0; fi
        _prev=$_cur
        _i=$((_i + 1))
    done
    echo "wait-stable: session '$_ss' never settled in ${_st}s" >&2
    exit 1
}

# wait-dead <session> [timeout-s] — poll #{pane_dead} until 1, then print
# #{pane_dead_status} (the pane command's exit status; tmux >= 2.2).
wait_dead() {
    _ds=$1; _dt=${2:-10}
    _i=0; _dmax=$((_dt * 5))
    while [ "$_i" -lt "$_dmax" ]; do
        if [ "$(t display-message -p -t "$_ds" '#{pane_dead}')" = "1" ]; then
            t display-message -p -t "$_ds" '#{pane_dead_status}'
            return 0
        fi
        sleep 0.2
        _i=$((_i + 1))
    done
    echo "wait-dead: pane in '$_ds' still alive after ${_dt}s" >&2
    snap "$_ds" >&2 || true
    exit 1
}

# stop <session> — kill-session, idempotent.
stop() { t kill-session -t "$1" 2>/dev/null || true; }

# killall — kill-server on THIS RUN's socket only, idempotent.
killall() { t kill-server 2>/dev/null || true; }

# CLI dispatch (dev convenience) — active only when executed, not sourced.
if [ "$(basename -- "$0")" = "tmux-drive.sh" ]; then
    _c=${1:?usage: tmux-drive.sh <command> [args...]}; shift
    case $_c in
        start)         start "$@" ;;
        start-wcartel) start_wcartel "$@" ;;
        snap)          snap "$@" ;;
        keys)          keys "$@" ;;
        type)          type_text "$@" ;;
        wait-for)      wait_for "$@" ;;
        wait-stable)   wait_stable "$@" ;;
        wait-dead)     wait_dead "$@" ;;
        stop)          stop "$@" ;;
        killall)       killall ;;
        *) echo "tmux-drive.sh: unknown command: $_c" >&2; exit 2 ;;
    esac
fi
```

- [ ] **Step 2: Write `scripts/smoke/clean-stale.sh`**

```sh
#!/bin/sh
# clean-stale.sh — sweep abandoned wcartel-smoke-* tmux servers left by
# crashed/interrupted runs. Sweeps ONLY sockets matching the smoke glob in the
# current user's tmux socket dir — never the user's own tmux servers.
set -u
dir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
if [ ! -d "$dir" ]; then
    echo "clean-stale: no tmux socket dir ($dir) — nothing to sweep"
    exit 0
fi
n=0
for sock in "$dir"/wcartel-smoke-*; do
    [ -e "$sock" ] || continue
    name=$(basename -- "$sock")
    tmux -L "$name" kill-server 2>/dev/null || true
    rm -f "$sock"
    n=$((n + 1))
    echo "clean-stale: swept $name"
done
echo "clean-stale: $n socket(s) swept"
```

- [ ] **Step 3: Make both executable**

Run: `chmod +x scripts/smoke/tmux-drive.sh scripts/smoke/clean-stale.sh`

- [ ] **Step 4: Exercise the helper end-to-end (no wcartel needed yet)**

Run, one line at a time:

```sh
export SMOKE_SOCKET="wcartel-smoke-dev$$"
scripts/smoke/tmux-drive.sh start t1 -- sh -c 'printf hello-smoke; sleep 30'
scripts/smoke/tmux-drive.sh wait-for t1 'hello-smoke'
scripts/smoke/tmux-drive.sh snap t1 | head -1
scripts/smoke/tmux-drive.sh stop t1
scripts/smoke/tmux-drive.sh start t2 --cols 20 --rows 5 -- sh -c 'exit 7'
scripts/smoke/tmux-drive.sh wait-dead t2
scripts/smoke/tmux-drive.sh killall
unset SMOKE_SOCKET
```

Expected: `wait-for` returns silently (exit 0); `snap | head -1` prints `hello-smoke`; `wait-dead t2` prints `7` (the keeper-first boot makes `remain-on-exit` deterministic even for an instantly-exiting command); `killall` returns silently. Also verify hermeticity: `tmux ls 2>&1` (default socket) must show no `t1`/`t2`/`__keeper` sessions.

- [ ] **Step 5: Verify the failure path can fail (timeout plumbing)**

Run:

```sh
export SMOKE_SOCKET="wcartel-smoke-dev$$"
scripts/smoke/tmux-drive.sh start t3 -- sh -c 'printf real-screen; sleep 30'
scripts/smoke/tmux-drive.sh wait-for t3 'never-on-screen' 2; echo "exit=$?"
scripts/smoke/tmux-drive.sh killall
unset SMOKE_SOCKET
```

Expected: after ~2s, stderr shows `wait-for: timeout (2s) on session 't3' waiting for ERE: never-on-screen`, then the `---- final screen ----` dump containing `real-screen`, and `exit=1`.

- [ ] **Step 6: Verify clean-stale sweeps only smoke sockets**

Run:

```sh
SMOKE_SOCKET="wcartel-smoke-stale$$" scripts/smoke/tmux-drive.sh start zz -- sleep 600
scripts/smoke/clean-stale.sh
ls "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/" | grep wcartel-smoke; echo "leftover=$?"
```

Expected: `clean-stale: swept wcartel-smoke-stale<pid>` (count ≥1), then `leftover=1` (no smoke sockets remain). Your own tmux server (if any) is untouched.

- [ ] **Step 7: Commit**

```bash
git add scripts/smoke/tmux-drive.sh scripts/smoke/clean-stale.sh
git commit -m "$(cat <<'EOF'
smoke: vendor tmux-drive.sh helper + stale-socket sweeper

Per-run private socket, config-free server (-f /dev/null) with
set-clipboard on / remain-on-exit on / exit-empty off, pane-dead exit
assertions, hermetic start-wcartel with the notice barrier
(--no-barrier escape reserved for S8's relaunch, where the recovery
modal replaces the status row).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 2: Debug-only panic trigger (the only Rust change)

**Files:**
- Modify: `wordcartel/src/app.rs:1099-1100` (top of `pub fn reduce`'s body)

**Interfaces:**
- Consumes: Task 1's helper (for the scratch-tmux verification only).
- Produces: pressing **F12** while `WCARTEL_SMOKE_PANIC` is set (any value) panics on the main thread with message `WCARTEL_SMOKE_PANIC: deliberate smoke-test panic` — in debug builds only. S7 (Task 9) depends on this exact message text. All other keys are unaffected; release builds compile the check out.

- [ ] **Step 1: Insert the trigger at the pinned anchor**

In `wordcartel/src/app.rs`, `pub fn reduce` opens at line 1091 and its body at line 1099. Insert the block between the body-opening brace and the existing `// pending_mark intercepts the very next key…` comment (currently line 1100) — i.e. the trigger becomes the first statement of `reduce`, ahead of every interception branch (pending_mark, menu, palette, theme picker, file browser, modal, minibuffer). The exact edit:

```rust
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) -> bool {
    // PTY-smoke panic trigger (debug builds only): F12 while WCARTEL_SMOKE_PANIC
    // is set panics HERE — the first statement of reduce, ahead of every
    // overlay/modal/minibuffer interception branch, so it fires regardless of
    // app state; reduce runs on the main thread (the panic hook ignores other
    // threads). Press-only, matching the app's kind filtering, so key
    // repeat/release under enhanced keyboard protocols cannot double-fire.
    // The key-code comparison short-circuits before the env read; release
    // builds compile the whole check out and the var is inert.
    #[cfg(debug_assertions)]
    if let Msg::Input(Event::Key(key)) = &msg {
        if key.kind == crossterm::event::KeyEventKind::Press
            && key.code == crossterm::event::KeyCode::F(12)
            && std::env::var_os("WCARTEL_SMOKE_PANIC").is_some()
        {
            panic!("WCARTEL_SMOKE_PANIC: deliberate smoke-test panic");
        }
    }
    // pending_mark intercepts the very next key as the mark letter.
```

(`Event` is already imported at `app.rs:6`; `KeyEventKind`/`KeyCode` use the fully-qualified form matching existing code such as `app.rs:1683`. F12 is confirmed unbound in the default keymap, `keymap.rs:222-318`. Do NOT run `cargo fmt`.)

- [ ] **Step 2: Build the debug binary**

Run: `cargo build -p wordcartel`
Expected: success, no warnings.

- [ ] **Step 3: GATE — full test suite**

Run: `cargo test`
Expected: all suites green (`wordcartel-core` lib + oracle, `wordcartel` lib).

- [ ] **Step 4: GATE — workspace clippy**

Run: `cargo clippy --workspace --all-targets`
Expected: clean (zero warnings; `[workspace.lints.clippy] all = "deny"` is in force). The nested `if let` + inner-`if` shape is already used clippy-clean at `app.rs:1103-1104`.

- [ ] **Step 5: Verify the trigger fires in a scratch tmux pane (debug binary)**

Run, one line at a time:

```sh
export SMOKE_SOCKET="wcartel-smoke-dev$$"
export WCARTEL_BIN="${CARGO_TARGET_DIR:-$PWD/target}/debug/wcartel"
export SMOKE_STATE_HOME=$(mktemp -d)
WCARTEL_SMOKE_PANIC=1 scripts/smoke/tmux-drive.sh start-wcartel trig
scripts/smoke/tmux-drive.sh type trig "dump me"
scripts/smoke/tmux-drive.sh wait-for trig 'dump me'
scripts/smoke/tmux-drive.sh keys trig F12
scripts/smoke/tmux-drive.sh wait-dead trig
scripts/smoke/tmux-drive.sh snap trig | grep WCARTEL_SMOKE_PANIC
scripts/smoke/tmux-drive.sh killall
```

Expected: `start-wcartel` returns after the notice barrier; `wait-dead` prints `101` (Rust panic exit); `snap | grep` prints a line containing `WCARTEL_SMOKE_PANIC: deliberate smoke-test panic` (the chained default hook printed it AFTER the terminal restore, onto the dead pane's frozen screen). Also confirm `ls "$SMOKE_STATE_HOME/wordcartel/"` shows `recovered-scratch-<pid>-0.md` and that it contains `dump me`. The typed text is REQUIRED for the dump: `dump_on_panic()` (`recovery.rs:52-60`) dumps the `LAST_GOOD` snapshot, which is populated ONLY by the edit-apply path (`record_snapshot`, sole call site `editor.rs:263`) — with zero keystrokes there is NO dump, and that is correct behavior, not a bug. Do not touch `recovery.rs`/`term.rs`.

- [ ] **Step 6: Verify the release build ignores the trigger**

Run, one line at a time:

```sh
cargo build --release -p wordcartel
export WCARTEL_BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/wcartel"
WCARTEL_SMOKE_PANIC=1 scripts/smoke/tmux-drive.sh start-wcartel rel
scripts/smoke/tmux-drive.sh keys rel F12
scripts/smoke/tmux-drive.sh type rel "still alive"
scripts/smoke/tmux-drive.sh wait-for rel 'still alive'
scripts/smoke/tmux-drive.sh stop rel
scripts/smoke/tmux-drive.sh killall
rm -rf "$SMOKE_STATE_HOME"
unset SMOKE_SOCKET SMOKE_STATE_HOME WCARTEL_BIN
```

Expected: after F12 the app is still running — the typed `still alive` lands on screen and `wait-for` returns 0 (a positive liveness proof, not a sleep). No panic, var inert.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/app.rs
git commit -m "$(cat <<'EOF'
smoke: debug-only F12 panic trigger at the top of reduce

WCARTEL_SMOKE_PANIC + F12 panics on the main thread ahead of every
interception branch, so S7 can exercise the panic-hook restore +
recovery-dump path against the real binary. debug_assertions-gated:
release builds compile it out. cargo test + workspace clippy green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 3: Check S1 — startup & clean quit

**Files:**
- Create: `scripts/smoke/checks/s1-startup-quit.sh`

**Interfaces:**
- Consumes: Task 1 helper functions (`start_wcartel`, `wait_for`, `keys`, `wait_dead`, `stop`, `killall`); Task 2's debug binary (any check launch needs a built `target/debug/wcartel`).
- Produces: an executable check exiting 0/1; run.sh (Task 11) invokes it as `sh scripts/smoke/checks/s1-startup-quit.sh` with `SMOKE_SOCKET`/`SMOKE_TMPDIR`/`WCARTEL_BIN` exported.

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s1-startup-quit.sh — S1: a bare launch renders the unnamed buffer as
# [1/2] *untitled* (launch buffer + always-installed *scratch*); C-q on clean
# buffers quits immediately (no modal) with exit status 0.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s1.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s1
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

start_wcartel "$S"
# Durable head: launch buffer plus the always-installed *scratch* → [1/2],
# unnamed buffer displays as *untitled*, default mode is [PREVIEW].
wait_for "$S" '\[1/2\] \*untitled\* \[PREVIEW\]'
# Clean buffers → C-q quits immediately, no modal; pane-dead status is the
# process exit code.
keys "$S" C-q
st=$(wait_dead "$S")
[ "$st" = "0" ] || { echo "s1: exit status '$st', want 0" >&2; exit 1; }
```

- [ ] **Step 2: Make executable and run against the built debug binary**

Run: `chmod +x scripts/smoke/checks/s1-startup-quit.sh && sh scripts/smoke/checks/s1-startup-quit.sh && sh scripts/smoke/checks/s1-startup-quit.sh && sh scripts/smoke/checks/s1-startup-quit.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage — prove the assertion can fail**

Edit the `wait_for` line: change the ERE `'\[1/2\] \*untitled\* \[PREVIEW\]'` to `'\[1/3\] \*untitled\* \[PREVIEW\]'`.
Run: `sh scripts/smoke/checks/s1-startup-quit.sh; echo "exit=$?"`
Expected FAIL output: after ~10s, `wait-for: timeout (10s) on session 's1' waiting for ERE: \[1/3\] \*untitled\* \[PREVIEW\]`, the `---- final screen ----` dump showing the real `[1/2] *untitled* [PREVIEW]` status line, then `exit=1`.

- [ ] **Step 4: Restore the correct ERE and re-run**

Revert the line to `'\[1/2\] \*untitled\* \[PREVIEW\]'`.
Run: `sh scripts/smoke/checks/s1-startup-quit.sh && sh scripts/smoke/checks/s1-startup-quit.sh && sh scripts/smoke/checks/s1-startup-quit.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s1-startup-quit.sh
git commit -m "$(cat <<'EOF'
smoke: S1 — startup + clean-quit check (pane-dead exit 0)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 4: Check S2 — open-error handling

**Files:**
- Create: `scripts/smoke/checks/s2-open-errors.sh`

**Interfaces:**
- Consumes: Task 1 helper (`start_wcartel`, `wait_for`, `stop`, `killall`, `t`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s2-open-errors.sh — S2: open-error handling asserts DURABLE status-head
# strings only. Startup-time status messages ('new file', permission errors)
# are asynchronously overwritten by the clipboard notice and MUST NOT be
# asserted (spec Robustness rule 1).
#   (a) nonexistent path → opened-as-new: [1/2] <basename> [PREVIEW]; alive.
#   (b) chmod-000 file → open fails; the app continues on the unnamed LAUNCH
#       buffer: [1/2] *untitled*; alive. Skipped when the file is still
#       readable (uid 0: chmod 000 is a no-op for root).
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s2.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s2
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# (a) nonexistent path → opened-as-new semantics (NotFound folds into an
# empty buffer carrying the path).
start_wcartel "$S" "$WORK/s2-new.md"
wait_for "$S" '\[1/2\] s2-new\.md \[PREVIEW\]'
[ "$(t display-message -p -t "$S" '#{pane_dead}')" = "0" ] \
    || { echo "s2: app died opening a nonexistent path" >&2; exit 1; }
stop "$S"

# (b) unreadable file → OpenError::Permission; fallback is the unnamed LAUNCH
# buffer (the *scratch* buffer is a separate second buffer, not the fallback).
DENIED="$WORK/s2-denied.md"
printf 'secret\n' > "$DENIED"
chmod 000 "$DENIED"
if cat "$DENIED" >/dev/null 2>&1; then
    echo "s2: chmod 000 file still readable (uid 0?) — skipping the permission sub-assertion"
else
    start_wcartel "$S" "$DENIED"
    wait_for "$S" '\[1/2\] \*untitled\* \[PREVIEW\]'
    [ "$(t display-message -p -t "$S" '#{pane_dead}')" = "0" ] \
        || { echo "s2: app died on a permission-denied open" >&2; exit 1; }
fi
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s2-open-errors.sh && sh scripts/smoke/checks/s2-open-errors.sh && sh scripts/smoke/checks/s2-open-errors.sh && sh scripts/smoke/checks/s2-open-errors.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output (on a root shell instead: the single skip line), then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit sub-check (b)'s `wait_for` ERE from `'\[1/2\] \*untitled\* \[PREVIEW\]'` to `'\[1/2\] s2-denied\.md \[PREVIEW\]'` (deliberately asserting the WRONG fallback — as if the denied file had opened).
Run: `sh scripts/smoke/checks/s2-open-errors.sh; echo "exit=$?"`
Expected FAIL output: `wait-for: timeout (10s) … ERE: \[1/2\] s2-denied\.md \[PREVIEW\]` plus a final-screen dump showing `[1/2] *untitled* [PREVIEW]`, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert to `'\[1/2\] \*untitled\* \[PREVIEW\]'`.
Run: `sh scripts/smoke/checks/s2-open-errors.sh && sh scripts/smoke/checks/s2-open-errors.sh && sh scripts/smoke/checks/s2-open-errors.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s2-open-errors.sh
git commit -m "$(cat <<'EOF'
smoke: S2 — open-error check (durable heads, root-guarded chmod probe)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 5: Check S3 — save round-trip

**Files:**
- Create: `scripts/smoke/checks/s3-save-roundtrip.sh`

**Interfaces:**
- Consumes: Task 1 helper (`start_wcartel` — its built-in barrier makes `Saved` race-free — `type_text`, `keys`, `wait_for`, `stop`, `killall`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s3-save-roundtrip.sh — S3: type into a fresh file, dirty marker appears,
# C-s saves ('Saved' is race-free only AFTER the notice barrier, which
# start_wcartel already enforced), marker gone (asserted POSITIVELY via the
# undirtied head), file on disk contains the sentence (containment, not
# equality — exact EOL policy is the app's concern).
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s3.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s3
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

DOC="$WORK/s3-note.md"
start_wcartel "$S" "$DOC"
type_text "$S" "the quick brown fox"
# Dirty marker: the display name gains a '*' prefix → '[1/2] *s3-note.md'.
wait_for "$S" '\[1/2\] \*s3-note\.md'
keys "$S" C-s
wait_for "$S" 'Saved'
# Marker gone, asserted positively: the undirtied head '[1/2] s3-note.md'
# cannot match while the '*' prefix is still present.
wait_for "$S" '\[1/2\] s3-note\.md'
grep -q 'the quick brown fox' "$DOC" \
    || { echo "s3: saved file lacks the typed sentence" >&2; exit 1; }
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s3-save-roundtrip.sh && sh scripts/smoke/checks/s3-save-roundtrip.sh && sh scripts/smoke/checks/s3-save-roundtrip.sh && sh scripts/smoke/checks/s3-save-roundtrip.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit the disk assertion: change `grep -q 'the quick brown fox' "$DOC"` to `grep -q 'the quick brown wolf' "$DOC"`.
Run: `sh scripts/smoke/checks/s3-save-roundtrip.sh; echo "exit=$?"`
Expected FAIL output: `s3: saved file lacks the typed sentence`, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert to `'the quick brown fox'`.
Run: `sh scripts/smoke/checks/s3-save-roundtrip.sh && sh scripts/smoke/checks/s3-save-roundtrip.sh && sh scripts/smoke/checks/s3-save-roundtrip.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s3-save-roundtrip.sh
git commit -m "$(cat <<'EOF'
smoke: S3 — save round-trip check (barrier-gated 'Saved', disk proof)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 6: Check S4 — dirty-quit modal

**Files:**
- Create: `scripts/smoke/checks/s4-dirty-quit-modal.sh`

**Interfaces:**
- Consumes: Task 1 helper (`start_wcartel`, `type_text`, `keys`, `wait_for`, `wait_dead`, `stop`, `killall`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s4-dirty-quit-modal.sh — S4: dirty C-q raises the multi-buffer prompt
# '{n} buffer(s) unsaved: [A]ll save · [R]eview each · [C]ancel'; 'c' cancels
# ("modal gone" asserted POSITIVELY: a typed char lands only if the modal
# released input — never a stable-then-absent check); C-q again → 'r' →
# per-buffer '{name}: [S]ave · [D]iscard · [C]ancel' → 'd' discards the last
# dirty buffer → the app exits 0.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s4.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s4
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

start_wcartel "$S"
type_text "$S" "unsaved words"
wait_for "$S" 'unsaved words'
keys "$S" C-q
wait_for "$S" 'unsaved: \[A\]ll save · \[R\]eview each · \[C\]ancel'
keys "$S" c
# Modal gone — positive proof: this character can only land in the buffer if
# the modal released key input.
type_text "$S" "zz"
wait_for "$S" 'unsaved wordszz'
keys "$S" C-q
wait_for "$S" 'unsaved: \[A\]ll save · \[R\]eview each · \[C\]ancel'
keys "$S" r
wait_for "$S" ': \[S\]ave · \[D\]iscard · \[C\]ancel'
keys "$S" d
st=$(wait_dead "$S")
[ "$st" = "0" ] || { echo "s4: exit status '$st', want 0" >&2; exit 1; }
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s4-dirty-quit-modal.sh && sh scripts/smoke/checks/s4-dirty-quit-modal.sh && sh scripts/smoke/checks/s4-dirty-quit-modal.sh && sh scripts/smoke/checks/s4-dirty-quit-modal.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit the positive modal-gone assertion: change `wait_for "$S" 'unsaved wordszz'` to `wait_for "$S" 'unsaved wordsqq'`.
Run: `sh scripts/smoke/checks/s4-dirty-quit-modal.sh; echo "exit=$?"`
Expected FAIL output: `wait-for: timeout (10s) … ERE: unsaved wordsqq` plus a final-screen dump showing `unsaved wordszz` in the buffer, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert to `'unsaved wordszz'`.
Run: `sh scripts/smoke/checks/s4-dirty-quit-modal.sh && sh scripts/smoke/checks/s4-dirty-quit-modal.sh && sh scripts/smoke/checks/s4-dirty-quit-modal.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s4-dirty-quit-modal.sh
git commit -m "$(cat <<'EOF'
smoke: S4 — dirty-quit modal check (cancel proven positively, then discard-quit)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 7: Check S5 — headless notice + OSC 52 → tmux buffer

**Files:**
- Create: `scripts/smoke/checks/s5-clipboard-osc52.sh`

**Interfaces:**
- Consumes: Task 1 helper (`start_wcartel` — its barrier IS the headless-notice assertion — `type_text`, `keys`, `wait_for`, `t` for `show-buffer`, `stop`, `killall`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s5-clipboard-osc52.sh — S5: the forced-headless launch env guarantees the
# once-only 'system clipboard unavailable' notice (start_wcartel's barrier IS
# that assertion). Then: type a sentence, select it with default-layer
# shift-arrow extension (Copy no-ops on an empty selection), C-c → 'Copied'
# AND the OSC 52 payload lands in the run-private tmux paste buffer
# (set-clipboard on), asserted with show-buffer equality.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s5.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s5
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

start_wcartel "$S"
type_text "$S" "copy me now"
wait_for "$S" 'copy me now'
# Select the full 11-char sentence backwards from the cursor with the
# default-layer selecting motions (keymap 'shift-left' → select_left).
keys "$S" S-Left S-Left S-Left S-Left S-Left S-Left S-Left S-Left S-Left S-Left S-Left
keys "$S" C-c
wait_for "$S" 'Copied'
# OSC 52 → tmux buffer. Poll (never a blind sleep): the buffer write can
# trail the 'Copied' status by a beat — the escape bytes flush to the pty on
# the same loop iteration but tmux stores them asynchronously.
i=0
while [ "$i" -lt 50 ]; do
    if [ "$(t show-buffer 2>/dev/null || true)" = "copy me now" ]; then break; fi
    sleep 0.2
    i=$((i + 1))
done
buf=$(t show-buffer 2>/dev/null || true)
[ "$buf" = "copy me now" ] \
    || { echo "s5: tmux paste buffer is '$buf', want 'copy me now'" >&2; exit 1; }
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s5-clipboard-osc52.sh && sh scripts/smoke/checks/s5-clipboard-osc52.sh && sh scripts/smoke/checks/s5-clipboard-osc52.sh && sh scripts/smoke/checks/s5-clipboard-osc52.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit both buffer comparisons: change the two `"copy me now"` comparison strings (inside the poll and the final assert) to `"copy me later"`.
Run: `sh scripts/smoke/checks/s5-clipboard-osc52.sh; echo "exit=$?"`
Expected FAIL output: after the ~10s poll, `s5: tmux paste buffer is 'copy me now', want 'copy me later'`, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert both strings to `"copy me now"`.
Run: `sh scripts/smoke/checks/s5-clipboard-osc52.sh && sh scripts/smoke/checks/s5-clipboard-osc52.sh && sh scripts/smoke/checks/s5-clipboard-osc52.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s5-clipboard-osc52.sh
git commit -m "$(cat <<'EOF'
smoke: S5 — headless notice + OSC 52 → tmux paste buffer check

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 8: Check S6 — tiny-terminal guard

**Files:**
- Create: `scripts/smoke/checks/s6-tiny-terminal.sh`

**Interfaces:**
- Consumes: Task 1 helper (`start_wcartel` with `--cols/--rows`, `type_text`, `wait_for`, `t` for `resize-window`/`display-message`, `stop`, `killall`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s6-tiny-terminal.sh — S6: normal render at 60x15, resize-window to 3x1
# (verified achievable on tmux 3.6b; trips both halves of the w<4||h<2 guard)
# → the clamped '...' notice, no crash; resize back → content restored (a
# real-terminal cousin of the Resize-blank regression class).
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s6.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s6
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# 60 cols is safe for the barrier: head '[1/2] *untitled* [PREVIEW] ' (27
# chars) + 'system clipboard unavailable' (28 chars) ends at column 55.
start_wcartel "$S" --cols 60 --rows 15
type_text "$S" "resize survivor"
wait_for "$S" 'resize survivor'
wait_for "$S" '\[1/2\]'
t resize-window -t "$S" -x 3 -y 1
# The guard paints the clamped notice — at 3 columns exactly '...'.
wait_for "$S" '^\.\.\.'
[ "$(t display-message -p -t "$S" '#{pane_dead}')" = "0" ] \
    || { echo "s6: app died on the 3x1 resize" >&2; exit 1; }
t resize-window -t "$S" -x 60 -y 15
wait_for "$S" 'resize survivor'
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s6-tiny-terminal.sh && sh scripts/smoke/checks/s6-tiny-terminal.sh && sh scripts/smoke/checks/s6-tiny-terminal.sh && sh scripts/smoke/checks/s6-tiny-terminal.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit the guard assertion: change `wait_for "$S" '^\.\.\.'` to `wait_for "$S" '^terminal too small'` (a notice string the app never prints).
Run: `sh scripts/smoke/checks/s6-tiny-terminal.sh; echo "exit=$?"`
Expected FAIL output: `wait-for: timeout (10s) … ERE: ^terminal too small` with a final-screen dump whose only visible content is `...`, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert to `'^\.\.\.'`.
Run: `sh scripts/smoke/checks/s6-tiny-terminal.sh && sh scripts/smoke/checks/s6-tiny-terminal.sh && sh scripts/smoke/checks/s6-tiny-terminal.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s6-tiny-terminal.sh
git commit -m "$(cat <<'EOF'
smoke: S6 — tiny-terminal guard check (3x1 resize, '...' notice, restore)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 9: Check S7 — panic → restore → recovery dump

**Files:**
- Create: `scripts/smoke/checks/s7-panic-recovery.sh`

**Interfaces:**
- Consumes: Task 2's trigger (F12 + `WCARTEL_SMOKE_PANIC`, message `WCARTEL_SMOKE_PANIC: deliberate smoke-test panic`); Task 1 helper (`start_wcartel` forwards the exported `WCARTEL_SMOKE_PANIC`, `type_text`, `keys`, `wait_for`, `wait_dead`, `snap`, `stop`, `killall`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s7-panic-recovery.sh — S7: launch the DEBUG binary with WCARTEL_SMOKE_PANIC,
# dirty the buffer (ordinary keys pass through the trigger), F12 → main-thread
# panic. Assert: (a) the pane dies nonzero AND its frozen final screen is the
# panic output, not a wrecked editor frame (alt screen was LEFT — safe to
# assert negatively because remain-on-exit froze the dead pane's screen);
# (b) the panic message is visible (the chained default hook prints to stderr
# AFTER the terminal restore); (c) exactly one recovered-*.md dump exists in
# the per-check state dir and contains the typed sentence.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s7.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s7
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export WCARTEL_SMOKE_PANIC=1   # forwarded into the launch env by start_wcartel
start_wcartel "$S"
type_text "$S" "precious unsaved text"
wait_for "$S" 'precious unsaved text'
keys "$S" F12
st=$(wait_dead "$S")
if [ -z "$st" ] || [ "$st" = "0" ]; then
    echo "s7: exit status '$st', want nonzero (panic)" >&2; exit 1
fi
# (a) alt screen LEFT: no editor status head on the dead pane's frozen screen.
if snap "$S" | grep -qE '\[1/2\]'; then
    echo "s7: editor status head still on screen — alt screen not restored" >&2
    snap "$S" >&2
    exit 1
fi
# (b) the chained default hook printed the panic after restore.
snap "$S" | grep -q 'WCARTEL_SMOKE_PANIC: deliberate smoke-test panic' \
    || { echo "s7: panic message not on the dead pane's screen" >&2; snap "$S" >&2; exit 1; }
# (c) exactly one recovery dump, containing the typed sentence. The launch
# buffer is path-less, so the dump is recovered-scratch-<pid>-0.md.
count=0; dump=""
for f in "$SMOKE_STATE_HOME/wordcartel/recovered-"*.md; do
    [ -e "$f" ] || continue
    count=$((count + 1)); dump=$f
done
[ "$count" -eq 1 ] \
    || { echo "s7: expected exactly 1 recovered-*.md, found $count" >&2; exit 1; }
grep -q 'precious unsaved text' "$dump" \
    || { echo "s7: recovery dump lacks the typed sentence" >&2; exit 1; }
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s7-panic-recovery.sh && sh scripts/smoke/checks/s7-panic-recovery.sh && sh scripts/smoke/checks/s7-panic-recovery.sh && sh scripts/smoke/checks/s7-panic-recovery.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0`.

- [ ] **Step 3: Sabotage**

Edit the dump-content assertion: change `grep -q 'precious unsaved text' "$dump"` to `grep -q 'precious unsaved gold' "$dump"`.
Run: `sh scripts/smoke/checks/s7-panic-recovery.sh; echo "exit=$?"`
Expected FAIL output: `s7: recovery dump lacks the typed sentence`, then `exit=1` (and no timeout — the pane really died, the dump really exists; only the sabotaged content assertion gates it red).

- [ ] **Step 4: Restore and re-run**

Revert to `grep -q 'precious unsaved text' "$dump"`.
Run: `sh scripts/smoke/checks/s7-panic-recovery.sh && sh scripts/smoke/checks/s7-panic-recovery.sh && sh scripts/smoke/checks/s7-panic-recovery.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s7-panic-recovery.sh
git commit -m "$(cat <<'EOF'
smoke: S7 — panic → terminal restore → recovery-dump check

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 10: Check S8 — hard-kill → swap → recovery offer

**Files:**
- Create: `scripts/smoke/checks/s8-kill-swap-recovery.sh`

**Interfaces:**
- Consumes: Task 1 helper — including the `--no-barrier` flag of `start_wcartel` (this check's relaunch is its sole user in the suite) — and the swap naming pinned in Resolutions item 2 (`<state>/wordcartel/s8-doc.md-<16hex>.swp`).
- Produces: executable check exiting 0/1, invoked by run.sh (Task 11).

- [ ] **Step 1: Write the check**

```sh
#!/bin/sh
# s8-kill-swap-recovery.sh — S8 (the charter's data-loss check): type into a
# named doc, POLL the per-check XDG_STATE_HOME for the idle swap writer's file
# (a filesystem wait-for, never a blind 2s sleep; T_IDLE_MS = 2000), then
# kill-session — the app has no signal handling, so it dies with no cleanup
# and the swap survives. Relaunch on the SAME path → the open-time
# swap-recovery prompt appears on a real screen → 'r' recovers the sentence.
# The relaunch uses --no-barrier: the modal prompt replaces the status row, so
# the clipboard notice cannot appear until the prompt resolves — the prompt
# wait below is this launch's barrier.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s8.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s8
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

DOC="$WORK/s8-doc.md"
start_wcartel "$S" "$DOC"
type_text "$S" "words worth recovering"
wait_for "$S" 'words worth recovering'
# Filesystem wait-for on the swap file: named docs swap to
# <XDG_STATE_HOME>/wordcartel/<basename>-<fnv1a64(realpath) as 16 hex>.swp.
SWAP=""
i=0
while [ "$i" -lt 100 ]; do
    for f in "$SMOKE_STATE_HOME/wordcartel/s8-doc.md-"*.swp; do
        [ -e "$f" ] && SWAP=$f
    done
    [ -n "$SWAP" ] && break
    sleep 0.2
    i=$((i + 1))
done
[ -n "$SWAP" ] \
    || { echo "s8: swap file never appeared under $SMOKE_STATE_HOME/wordcartel/" >&2; exit 1; }
# Hard kill: destroy the session and its pty; no signal handling → no cleanup.
stop "$S"
[ -e "$SWAP" ] || { echo "s8: swap file vanished after the kill" >&2; exit 1; }
# Relaunch on the SAME path (DOC was never saved, so the file is absent on
# disk → assess() sees a swap with no matching file → Prompt).
start_wcartel "$S" --no-barrier "$DOC"
wait_for "$S" 'Recovery file found: \[R\]ecover · \[D\]iscard · \[O\]pen original'
keys "$S" r
wait_for "$S" 'words worth recovering'
```

- [ ] **Step 2: Make executable and run**

Run: `chmod +x scripts/smoke/checks/s8-kill-swap-recovery.sh && sh scripts/smoke/checks/s8-kill-swap-recovery.sh && sh scripts/smoke/checks/s8-kill-swap-recovery.sh && sh scripts/smoke/checks/s8-kill-swap-recovery.sh; echo "exit=$?"   # 3 consecutive green runs (chained — any failure propagates); per-check flake surfaces HERE, not in Task 13`
Expected PASS output: no output, then `exit=0` (the swap poll adds ~2s — the idle debounce).

- [ ] **Step 3: Sabotage**

Edit the recovery-prompt assertion: change the ERE `'Recovery file found: \[R\]ecover · \[D\]iscard · \[O\]pen original'` to `'Recovery file found: \[R\]ecover · \[K\]eep both · \[O\]pen original'` (a prompt the app never shows).
Run: `sh scripts/smoke/checks/s8-kill-swap-recovery.sh; echo "exit=$?"`
Expected FAIL output: `wait-for: timeout (10s) … ERE: Recovery file found: \[R\]ecover · \[K\]eep both · \[O\]pen original` with a final-screen dump showing the REAL prompt `Recovery file found: [R]ecover · [D]iscard · [O]pen original`, then `exit=1`.

- [ ] **Step 4: Restore and re-run**

Revert the ERE to `'Recovery file found: \[R\]ecover · \[D\]iscard · \[O\]pen original'`.
Run: `sh scripts/smoke/checks/s8-kill-swap-recovery.sh && sh scripts/smoke/checks/s8-kill-swap-recovery.sh && sh scripts/smoke/checks/s8-kill-swap-recovery.sh; echo "exit=$?"   # 3x again after restoring (chained)`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add scripts/smoke/checks/s8-kill-swap-recovery.sh
git commit -m "$(cat <<'EOF'
smoke: S8 — hard-kill → swap → recovery-offer check (filesystem swap poll)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 11: Runner (`run.sh`) + gitignored history

**Files:**
- Create: `scripts/smoke/run.sh`
- Modify: `.gitignore` (append the history entry)

**Interfaces:**
- Consumes: all eight checks (Tasks 3–10) at `scripts/smoke/checks/s[1-8]-*.sh`; the env contract (`SMOKE_SOCKET`, `SMOKE_TMPDIR`, `WCARTEL_BIN` exported to checks).
- Produces: `sh scripts/smoke/run.sh` — exit 0 all-green or environment-skip; exit 1 on any check failure or build failure; per-check `PASS s<N>` / `FAIL s<N>` lines; a one-line summary (`smoke: 8/8 PASS` or `smoke: FAIL s<N> — advisory (…)`) printed AND appended to `scripts/smoke/.history`. This summary line is what CLAUDE.md (Task 12) requires quoting in pre-merge reports.

- [ ] **Step 1: Write `scripts/smoke/run.sh`**

```sh
#!/bin/sh
# run.sh — wordcartel PTY smoke suite. Runs S1–S8 against the real debug
# binary in a private per-run tmux server. ADVISORY layer: running it is a
# mandatory pre-merge step, but a red result never blocks a merge (see
# CLAUDE.md). Rerun-safe; never touches the user's tmux sockets or the real
# ~/.local/state/wordcartel.
set -u

# --- pre-flight FIRST, builtins only (command/echo/exit) — so a PATH with no
# external tools still reaches the skip notice cleanly. tmux >= 3.0 required
# (resize-window needs >= 2.9; inner-OSC-52-to-buffer semantics are stable in
# 3.x). Either check failing prints a one-line skip notice and exits 0 — skip
# is exit 0 by design (advisory).
if ! command -v tmux >/dev/null 2>&1; then
    echo "smoke: SKIP — tmux not installed"
    exit 0
fi

SMOKE_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SMOKE_DIR/../.." && pwd)
ver=$(tmux -V | sed 's/^tmux[^0-9]*//')
major=${ver%%.*}
case $major in
    ''|*[!0-9]*) echo "smoke: SKIP — cannot parse tmux version ('$ver')"; exit 0 ;;
esac
if [ "$major" -lt 3 ]; then
    echo "smoke: SKIP — tmux >= 3.0 required (found $ver)"
    exit 0
fi

# --- per-run identity: unique socket + temp tree, exported to the checks.
SMOKE_SOCKET="wcartel-smoke-$$"
export SMOKE_SOCKET
RUN_DIR=$(mktemp -d "${TMPDIR:-/tmp}/wcartel-smoke-run.XXXXXX")
SMOKE_TMPDIR="$RUN_DIR"
export SMOKE_TMPDIR
# Resolve the binary ONCE (a bare target/debug/ path breaks under
# CARGO_TARGET_DIR; if set, it must be absolute — cargo convention).
WCARTEL_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel"
export WCARTEL_BIN

cleanup() {
    tmux -L "$SMOKE_SOCKET" kill-server 2>/dev/null || true
    rm -rf "$RUN_DIR"
}
trap cleanup EXIT

# --- build (debug: S7 requires debug_assertions). Cargo output goes to a log
# under the run's temp dir so it stays out of the transcript and session
# captures; printed in full only on failure (pinned resolution 3).
if ! (cd "$REPO_ROOT" && cargo build -p wordcartel) >"$RUN_DIR/build.log" 2>&1; then
    echo "smoke: cargo build failed — full log:" >&2
    cat "$RUN_DIR/build.log" >&2
    exit 1
fi
if [ ! -x "$WCARTEL_BIN" ]; then
    echo "smoke: built OK but no binary at $WCARTEL_BIN" >&2
    exit 1
fi

# --- run S1–S8 sequentially; per-check PASS/FAIL lines.
pass=0; fail=0; first_fail=""
for check in "$SMOKE_DIR"/checks/s[1-8]-*.sh; do
    short=$(basename -- "$check")
    short=${short%%-*}
    if sh "$check" >"$RUN_DIR/$short.log" 2>&1; then
        echo "PASS $short"
        pass=$((pass + 1))
    else
        echo "FAIL $short"
        sed "s/^/  $short| /" "$RUN_DIR/$short.log"
        fail=$((fail + 1))
        if [ -z "$first_fail" ]; then first_fail=$short; fi
    fi
done

# --- one-line summary: printed AND appended to the (gitignored) history file
# so the stability record accumulates across runs.
total=$((pass + fail))
if [ "$fail" -eq 0 ]; then
    summary="smoke: $pass/$total PASS"
else
    summary="smoke: FAIL $first_fail — advisory ($pass/$total passed)"
fi
echo "$summary"
printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "$summary" >> "$SMOKE_DIR/.history"
if [ "$fail" -gt 0 ]; then exit 1; fi
exit 0
```

- [ ] **Step 2: Gitignore the history file**

Append to `/home/jkeim/projects/groundwords/.gitignore`:

```
# PTY smoke suite stability record (scripts/smoke/run.sh)
scripts/smoke/.history
```

- [ ] **Step 3: Make executable and run the full suite**

Run: `chmod +x scripts/smoke/run.sh && sh scripts/smoke/run.sh; echo "exit=$?"`
Expected PASS output:

```
PASS s1
PASS s2
PASS s3
PASS s4
PASS s5
PASS s6
PASS s7
PASS s8
smoke: 8/8 PASS
exit=0
```

and `tail -1 scripts/smoke/.history` shows a timestamped `smoke: 8/8 PASS` line. Verify `git status` shows `.history` as ignored (not untracked).

- [ ] **Step 4: Verify the skip path (no tmux on PATH)**

Run:

```sh
mkdir -p /tmp/smoke-emptybin
env PATH=/tmp/smoke-emptybin /bin/sh scripts/smoke/run.sh; echo "exit=$?"
```

Expected: `smoke: SKIP — tmux not installed`, then `exit=0` (skip is exit 0 by design — the layer is advisory).

- [ ] **Step 5: Sabotage — verify a red run exits 1 with the advisory summary**

Temporarily edit `scripts/smoke/checks/s5-clipboard-osc52.sh`: change the final comparison `[ "$buf" = "copy me now" ]` to `[ "$buf" = "copy me never" ]`.
Run: `sh scripts/smoke/run.sh; echo "exit=$?"`
Expected: `PASS s1` … `PASS s4`, `FAIL s5` followed by the indented `s5| …` log lines, `PASS s6` … `PASS s8`, then `smoke: FAIL s5 — advisory (7/8 passed)` and `exit=1`; `.history` gains that advisory line.

- [ ] **Step 6: Restore and re-run**

Revert s5's comparison to `[ "$buf" = "copy me now" ]`.
Run: `sh scripts/smoke/run.sh; echo "exit=$?"`
Expected: `smoke: 8/8 PASS`, `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add scripts/smoke/run.sh .gitignore
git commit -m "$(cat <<'EOF'
smoke: run.sh suite runner + gitignored .history stability record

Pre-flight (tmux >= 3.0, skip = exit 0), per-run socket + temp tree,
debug build logged to the run dir (printed only on failure), S1-S8
with PASS/FAIL lines, one-line summary appended to .history.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 12: CLAUDE.md — mandatory-run/advisory-pass policy + frontier amend

**Files:**
- Modify: `CLAUDE.md:93-94` (insert a paragraph after the GATEs bullet list, before the `**Formatting…**` paragraph)
- Modify: `CLAUDE.md:154-155` (the hardening-campaign "New candidate" frontier note)

**Interfaces:**
- Consumes: the summary-line format produced by run.sh (Task 11): `smoke: 8/8 PASS` / `smoke: FAIL s<N> — advisory (…)`.
- Produces: repo policy text; no code depends on it.

- [ ] **Step 1: Insert the smoke-suite policy subsection**

In `CLAUDE.md`, directly after the GATEs bullet list (currently ending at line 93 with `- New code matches the surrounding **house style** (see below) by review.`) and before the `**Formatting — do NOT run `cargo fmt`.**` paragraph, insert this exact text (blank line above and below):

```markdown
**PTY smoke suite — mandatory-run, advisory-pass (NOT a GATE):** every effort's pre-merge
report MUST run `scripts/smoke/run.sh` and quote its one-line summary verbatim (e.g.
`smoke: 8/8 PASS`). A red result NEVER blocks a merge — it is an advisory finding that
must be surfaced to the human explicitly (e.g. `smoke: FAIL s5 — advisory`). `cargo test`
+ workspace clippy remain the only merge gates. The suite drives the real `wcartel`
binary in a private per-run tmux server (`scripts/smoke/`, checks S1–S8); a skip on a
tmux-less machine (`smoke: SKIP — …`) is quoted the same way. Promotion to a gate later
is an edit to this paragraph, contingent on the stability record accumulating in the
gitignored `scripts/smoke/.history`.
```

- [ ] **Step 2: Amend the hardening-campaign frontier note**

In `CLAUDE.md` (currently lines 148–155), replace this exact text:

```markdown
*isolates* its parse panic). **New candidate:** an e2e/TUI harness (the live `wcartel`
binary has no end-to-end/interactive test coverage — the campaign's one untouched frontier).
```

with this exact text:

```markdown
*isolates* its parse panic). **e2e/TUI frontier — now covered by two layers:** in-process
journeys drive the real `reduce → advance → render` loop against a `TestBackend`
(`wordcartel/src/e2e.rs`); the PTY smoke suite (`scripts/smoke/`, mandatory-run /
advisory-pass — see the Rust-conventions note above) drives the live `wcartel` binary in
a private tmux server: startup/exit codes, open errors, real-terminal save, dirty-quit
modal, OSC 52 → tmux buffer, tiny-terminal guard, panic → restore → recovery dump, and
hard-kill → swap → recovery.
```

- [ ] **Step 3: Verify the doc edits render sanely**

Run: `grep -n "PTY smoke suite" CLAUDE.md && grep -n "now covered by two layers" CLAUDE.md`
Expected: one hit each — the policy paragraph inside the Rust-conventions GATE area, the amended note inside the Hardening-campaign section. Read both paragraphs in place to confirm no broken markdown.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: smoke suite is mandatory-run/advisory-pass; amend e2e frontier note

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: 98fc57e3-bc67-4e8a-9e95-95995a1ce9c7
EOF
)"
```

---

### Task 13: Burn-in (≥20 consecutive green runs) + final verification

**Files:**
- None created or modified (evidence lands in the gitignored `scripts/smoke/.history`).

**Interfaces:**
- Consumes: the complete suite (Tasks 1–11) and the policy (Task 12).
- Produces: the effort's done-declaration evidence: ≥20 consecutive `smoke: 8/8 PASS` history lines, plus the spec's remaining verification items (gates re-run, environment untouched).

- [ ] **Step 1: Snapshot the pre-burn-in environment state**

Run:

```sh
ls -A ~/.local/state/wordcartel/ > /tmp/smoke-state-before.txt 2>&1 || echo "absent" > /tmp/smoke-state-before.txt   # names only — ls -la's dir mtimes flip from unrelated sibling activity
tmux ls > /tmp/smoke-tmux-before.txt 2>&1 || echo "no server" > /tmp/smoke-tmux-before.txt
```

- [ ] **Step 2: Run the burn-in loop**

Run:

```sh
n=0
while [ "$n" -lt 20 ]; do
    sh scripts/smoke/run.sh || break
    n=$((n + 1))
    echo "== green runs so far: $n =="
done
echo "green runs: $n"
```

Expected: `green runs: 20` (each iteration printing `PASS s1` … `PASS s8`, `smoke: 8/8 PASS`). If ANY run fails: STOP, use superpowers:systematic-debugging on the flake, fix, and restart the count at 0 — one green run proves plumbing, not stability, and stability is the suite's entire charter (never cry wolf).

- [ ] **Step 3: Confirm the history-file evidence**

Run: `tail -n 20 scripts/smoke/.history`
Expected: 20 consecutive timestamped `smoke: 8/8 PASS` lines (no FAIL lines interleaved).

- [ ] **Step 4: Re-run the merge gates (spec Verification 4)**

Run: `cargo test && cargo clippy --workspace --all-targets`
Expected: tests green, clippy clean.

- [ ] **Step 5: Verify the developer's environment is untouched (spec Verification 5)**

Run:

```sh
{ ls -A ~/.local/state/wordcartel/ 2>&1 || echo absent; } > /tmp/smoke-state-after.txt
cmp -s /tmp/smoke-state-after.txt /tmp/smoke-state-before.txt && echo "state-dir: untouched"
{ tmux ls 2>&1 || echo "no server"; } > /tmp/smoke-tmux-after.txt
cmp -s /tmp/smoke-tmux-after.txt /tmp/smoke-tmux-before.txt && echo "user-tmux: untouched"
ls "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/" 2>/dev/null | grep wcartel-smoke; echo "stale-sockets-grep-exit=$?"
```

Expected: `state-dir: untouched`, `user-tmux: untouched`, and `stale-sockets-grep-exit=1` (no leftover smoke sockets; if any exist, `scripts/smoke/clean-stale.sh` sweeps them and the leak's origin must be found before declaring done).

- [ ] **Step 6: Pre-merge report**

Quote the final summary line (`smoke: 8/8 PASS`) and the burn-in evidence (`green runs: 20`) in the effort's pre-merge report, per the CLAUDE.md policy added in Task 12. No commit — burn-in changes no tracked files.

---

## Spec-Coverage Table

Self-review complete (coverage sweep, placeholder scan — no TBD/"similar to"/elided code blocks; name-consistency pass: function names `start`/`start_wcartel`/`snap`/`keys`/`type_text`/`wait_for`/`wait_stable`/`wait_dead`/`stop`/`killall`/`t`/`boot`, env vars `SMOKE_SOCKET`/`SMOKE_TMPDIR`/`SMOKE_STATE_HOME`/`WCARTEL_BIN`/`WCARTEL_SMOKE_PANIC`, and check filenames are identical across Tasks 1–13 and the run.sh glob `s[1-8]-*.sh`).

| Spec section | Where in this plan |
|---|---|
| Context / resolved design forks (vendored helper; standalone runner not a GATE; debug trigger) | Global Constraints; Tasks 1, 2, 11, 12 |
| Goals — helper vocabulary on a private per-run socket | Task 1 |
| Goals — eight seed checks S1–S8 | Tasks 3–10 |
| Goals — `debug_assertions` panic trigger | Task 2 |
| Goals — run.sh (per-check PASS/FAIL, nonzero exit, graceful skip) | Task 11 |
| Goals — CLAUDE.md mandatory-run/advisory-pass | Task 12 |
| Non-goals (no gate, no journey duplication, no release change, no CI, no general library) | Global Constraints; no task exceeds them |
| Component 1 — command table (start/start-wcartel/snap/keys/type/wait-for/wait-stable/wait-dead/stop/killall) | Task 1 Step 1 |
| Component 1 — pane-dead exit assertions (`remain-on-exit`, `#{pane_dead_status}`) | Task 1 (helper `boot`/`wait_dead`); Tasks 3, 6, 9 use it |
| Component 1 — per-run socket + `clean-stale.sh` | Task 1 (sweeper), Task 11 (socket generation), Task 13 Step 5 (leak audit) |
| Component 1 — hermeticity rule 1 (`-f /dev/null`, `set-clipboard on`) | Task 1 (`t` wrapper + `boot`) |
| Component 1 — hermeticity rule 2 (env -u TMUX/DISPLAY/WAYLAND_DISPLAY, XDG_STATE_HOME tempdir) | Task 1 (`start_wcartel`) |
| Component 1 — hermeticity rule 3 (`--no-config`, default keymap) | Task 1 (`start_wcartel`); asserted keys in Tasks 3–10 |
| Component 2 — S1 startup & clean quit | Task 3 |
| Component 2 — S2 open-error handling (durable heads, root guard) | Task 4 |
| Component 2 — S3 save round-trip (barrier first) | Task 5 |
| Component 2 — S4 dirty-quit modal (positive modal-gone) | Task 6 |
| Component 2 — S5 headless notice + OSC 52 → tmux buffer (shift-arrow selection) | Task 7 |
| Component 2 — S6 tiny-terminal guard (3×1, restore) | Task 8; Pinned Resolution 4 |
| Component 2 — S7 panic → restore → recovery dump | Task 9 |
| Component 2 — S8 hard-kill → swap → recovery (filesystem swap poll) | Task 10; Pinned Resolutions 2 and 5 |
| Component 3 — trigger placement (main thread, before interception, Press-only, F12 unbound, clippy-clean) | Task 2; Pinned Resolution 1 |
| Component 4 — run.sh (pre-flight, WCARTEL_BIN resolution, build handling, summary, .history, rerun-safe) | Task 11; Pinned Resolution 3 |
| Component 4 — CLAUDE.md subsection + frontier amend | Task 12 (exact edited text shown) |
| Robustness — no bare sleeps / wait-for on pinned strings / positive re-expression | helper (Task 1); every check (Tasks 3–10); S5/S8 polls |
| Robustness — `wait-stable` is a settling aid only | Task 1 (documented in the function comment; no check uses it as an assertion) |
| Robustness — trap cleanup per check | boilerplate `cleanup()`/`trap` in Tasks 3–10 |
| Robustness — startup-status race, rules 1 and 2 (durable heads; notice barrier) | Task 4 (rule 1); Task 1 barrier + Tasks 5, 7 (rule 2) |
| Verification 1 — full suite passes | Task 11 Step 3 |
| Verification 1b — burn-in ≥20 + history evidence | Task 13 Steps 2–3 |
| Verification 2 — sabotage per check | Tasks 3–10, sabotage/restore steps; runner-level in Task 11 Step 5 |
| Verification 3 — release build ignores the var | Task 2 Step 6 |
| Verification 4 — cargo test + clippy gates | Task 2 Steps 3–4; re-run Task 13 Step 4 |
| Verification 5 — developer environment untouched | Task 1 Steps 4/6; Task 13 Steps 1, 5 |
| Open items (anchor; build output; — and the S6/S8 pins requested at plan time) | Pinned Resolutions 1–5 |
