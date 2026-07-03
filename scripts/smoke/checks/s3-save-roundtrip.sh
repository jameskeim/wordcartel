#!/bin/sh
# s3-save-roundtrip.sh — S3: type into a fresh file, dirty marker appears,
# C-s saves ('Saved' is race-free only AFTER the notice barrier, which
# start_wcartel already enforced), marker gone (asserted POSITIVELY via the
# undirtied head), file on disk contains the sentence (containment, not
# equality — exact EOL policy is the app's concern).
set -eu
CHECK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$CHECK_DIR/../../.." && pwd)
: "${WCARTEL_BIN:=${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel}"
export WCARTEL_BIN
if [ -n "${SMOKE_SOCKET:-}" ]; then OWN_SERVER=0; else
    SMOKE_SOCKET="wcartel-smoke-$$"; OWN_SERVER=1
fi
export SMOKE_SOCKET
WORK=$(mktemp -d "${SMOKE_TMPDIR:-${TMPDIR:-/tmp}}/s3.XXXXXX")
SMOKE_STATE_HOME="$WORK/state"; mkdir -p "$SMOKE_STATE_HOME"; export SMOKE_STATE_HOME
. "$CHECK_DIR/../tmux-drive.sh"
S=s3
cleanup() {
    stop "$S"
    if [ "$OWN_SERVER" = "1" ]; then killall; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

DOC="$WORK/s3-note.md"
start_wcartel "$S" "$DOC"
type_text "$S" "the quick brown fox"
# Dirty marker: the display name gains a '*' prefix → '[1/2] *s3-note.md'.
wait_for "$S" '\[1/2\] \*s3-note\.md'
keys "$S" C-s
wait_for "$S" 'Saved'
# Marker gone, asserted positively: the undirtied head '[1/2] s3-note.md'
# cannot match while the '*' prefix is still present.
wait_for "$S" '\[1/2\] s3-note\.md'
grep -q 'the quick brown fox' "$DOC" \
    || { echo "s3: saved file lacks the typed sentence" >&2; exit 1; }
