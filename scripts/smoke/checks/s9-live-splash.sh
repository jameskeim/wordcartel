#!/bin/sh
# s9-live-splash.sh — S9 (the live-binary counterpart to e2e.rs's
# e2e_splash_first_frame_then_key_dismisses_and_is_consumed): launch WITH the
# real startup splash (--with-splash --no-barrier — the splash owns the
# screen, so the default '[1/' barrier can't match yet), assert the real
# first-frame strings from splash.rs (WORDMARK/TAGLINE/FOOTER), press a key to
# dismiss, then assert the editor is revealed ('[1/' appears) and the footer
# text is gone.
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s9.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s9
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

start_wcartel "$S" --with-splash --no-barrier
# First frame: the real splash strings (splash.rs WORDMARK/TAGLINE/FOOTER).
wait_for "$S" 'wordcartel'
wait_for "$S" 'Everyone needs a cover story'
wait_for "$S" 'press any key'
if snap "$S" | grep -qE '\[1/'; then
    echo "s9: buffer indicator visible before dismiss — splash does not own the screen" >&2
    exit 1
fi
# Any key dismisses AND is consumed; the editor is revealed underneath.
keys "$S" x
wait_for "$S" '\[1/'
if snap "$S" | grep -qE 'press any key'; then
    echo "s9: splash footer still visible after dismiss" >&2
    exit 1
fi
