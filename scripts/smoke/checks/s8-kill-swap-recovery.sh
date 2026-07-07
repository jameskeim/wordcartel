#!/bin/sh
# s8-kill-swap-recovery.sh — S8 (the charter's data-loss check): type into a
# named doc, POLL the per-check XDG_STATE_HOME for the idle swap writer's file
# (a filesystem wait-for, never a blind 2s sleep; T_IDLE_MS = 2000), then
# kill-session — the app has no signal handling, so it dies with no cleanup
# and the swap survives. Relaunch on the SAME path → the open-time
# swap-recovery prompt appears on a real screen → 'r' recovers the sentence.
# The relaunch uses --no-barrier: the modal prompt replaces the status row, so
# the '[1/' buffer-indicator barrier cannot appear until the prompt resolves —
# the prompt wait below is this launch's own, stronger barrier.
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
