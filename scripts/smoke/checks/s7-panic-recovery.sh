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
