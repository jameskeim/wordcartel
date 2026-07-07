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
#                                    the app runs the OSC-52 fallback path
#   XDG_STATE_HOME=$SMOKE_STATE_HOME — swap/recovery land in a per-check tempdir
# plus --no-config (checks assert against the DEFAULT keymap). Forwards
# WCARTEL_SMOKE_PANIC when the caller has it set (S7). Blocks on the buffer
# indicator '[1/' barrier before returning — the status bar's launch-invariant
# chrome, present on the first frame of EVERY launch (no-arg and doc) regardless
# of clipboard behavior. On this headless path the app reports the clipboard
# AVAILABLE (Null Layer-1 + bare OSC 52), so no async status write follows, and
# dispatch-set statuses (Saved/Copied) are race-free. --no-barrier exists for
# exactly ONE launch in the suite — S8's relaunch, where the swap-recovery
# modal replaces the status row (render.rs status branch); that check's wait-for
# on the prompt text is its own, stronger barrier.
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
        wait_for "$_sess" '\[1/'
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
