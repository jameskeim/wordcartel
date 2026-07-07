#!/bin/sh
# s2-open-errors.sh — S2: open-error handling asserts DURABLE status-head
# strings only. Transient startup-time status messages ('new file', permission
# errors) can be overwritten and MUST NOT be asserted (spec Robustness rule 1);
# the durable '[PREVIEW]' / buffer-indicator head is what we assert.
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
