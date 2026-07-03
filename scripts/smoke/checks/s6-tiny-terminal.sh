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
