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
