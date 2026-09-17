#!/bin/sh
# S8: checkpoint a named document, terminate its process, then select the
# abandoned checkpoint in the recovery picker and open a separate document.
# Filesystem and screen barriers have finite deadlines; no blind startup sleeps.
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
# Wait for the independently owned checkpoint.
SWAP=""
i=0
while [ "$i" -lt 100 ]; do
    for f in "$SMOKE_STATE_HOME/wordcartel/recovery-v2/"*/checkpoint.wcr; do
        [ -e "$f" ] && SWAP=$f
    done
    [ -n "$SWAP" ] && break
    sleep 0.2
    i=$((i + 1))
done
[ -n "$SWAP" ] \
    || { echo "s8: checkpoint never appeared under $SMOKE_STATE_HOME/wordcartel/" >&2; exit 1; }
# Hard kill: destroy the session and its pty; no signal handling → no cleanup.
stop "$S"
[ -e "$SWAP" ] || { echo "s8: checkpoint vanished after the kill" >&2; exit 1; }
# The named disk document remains empty; recovery opens alongside it.
start_wcartel "$S" --no-barrier "$DOC"
wait_for "$S" 'Review Recovery Files'
keys "$S" Space Enter
wait_for "$S" 'Recovery opening complete'
keys "$S" Escape
wait_for "$S" 'words worth recovering'
wait_for "$S" 'Recovered'
[ ! -e "$DOC" ] || { echo "s8: recovery unexpectedly wrote original file" >&2; exit 1; }
