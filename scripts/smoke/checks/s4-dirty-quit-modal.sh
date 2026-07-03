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
