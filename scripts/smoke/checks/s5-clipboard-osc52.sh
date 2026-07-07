#!/bin/sh
# s5-clipboard-osc52.sh — S5: on the forced-headless launch env the app reports
# the clipboard AVAILABLE (Null Layer-1 + bare OSC 52) and emits bare OSC 52 —
# start_wcartel's barrier is now the launch-invariant '[1/' buffer indicator,
# not a clipboard notice. Then: type a sentence, select it with default-layer
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
