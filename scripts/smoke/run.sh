#!/bin/sh
# run.sh — wordcartel PTY smoke suite. Runs S1–S9 against the real debug
# binary in a private per-run tmux server. ADVISORY layer: running it is a
# mandatory pre-merge step, but a red result never blocks a merge (see
# CLAUDE.md). Rerun-safe; never touches the user's tmux sockets or the real
# ~/.local/state/wordcartel.
set -u

# --- pre-flight FIRST, builtins only (command/echo/exit) — so a PATH with no
# external tools still reaches the skip notice cleanly. tmux >= 3.0 required
# (resize-window needs >= 2.9; inner-OSC-52-to-buffer semantics are stable in
# 3.x). Either check failing prints a one-line skip notice and exits 0 — skip
# is exit 0 by design (advisory).
if ! command -v tmux >/dev/null 2>&1; then
    echo "smoke: SKIP — tmux not installed"
    exit 0
fi

SMOKE_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SMOKE_DIR/../.." && pwd)
ver=$(tmux -V | sed 's/^tmux[^0-9]*//')
major=${ver%%.*}
case $major in
    ''|*[!0-9]*) echo "smoke: SKIP — cannot parse tmux version ('$ver')"; exit 0 ;;
esac
if [ "$major" -lt 3 ]; then
    echo "smoke: SKIP — tmux >= 3.0 required (found $ver)"
    exit 0
fi

# --- per-run identity: unique socket + temp tree, exported to the checks.
SMOKE_SOCKET="wcartel-smoke-$$"
export SMOKE_SOCKET
RUN_DIR=$(mktemp -d "${TMPDIR:-/tmp}/wcartel-smoke-run.XXXXXX")
SMOKE_TMPDIR="$RUN_DIR"
export SMOKE_TMPDIR
# Resolve the binary ONCE (a bare target/debug/ path breaks under
# CARGO_TARGET_DIR; if set, it must be absolute — cargo convention).
WCARTEL_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel"
export WCARTEL_BIN

cleanup() {
    tmux -L "$SMOKE_SOCKET" kill-server 2>/dev/null || true
    rm -rf "$RUN_DIR"
}
trap cleanup EXIT

# --- build (debug: S7 requires debug_assertions). Cargo output goes to a log
# under the run's temp dir so it stays out of the transcript and session
# captures; printed in full only on failure (pinned resolution 3).
if ! (cd "$REPO_ROOT" && cargo build -p wordcartel) >"$RUN_DIR/build.log" 2>&1; then
    echo "smoke: cargo build failed — full log:" >&2
    cat "$RUN_DIR/build.log" >&2
    exit 1
fi
if [ ! -x "$WCARTEL_BIN" ]; then
    echo "smoke: built OK but no binary at $WCARTEL_BIN" >&2
    exit 1
fi

# --- run S1–S9 sequentially; per-check PASS/FAIL lines.
pass=0; fail=0; first_fail=""
for check in "$SMOKE_DIR"/checks/s[1-9]-*.sh; do
    short=$(basename -- "$check")
    short=${short%%-*}
    if sh "$check" >"$RUN_DIR/$short.log" 2>&1; then
        echo "PASS $short"
        pass=$((pass + 1))
    else
        echo "FAIL $short"
        sed "s/^/  $short| /" "$RUN_DIR/$short.log"
        fail=$((fail + 1))
        if [ -z "$first_fail" ]; then first_fail=$short; fi
    fi
done

# --- one-line summary: printed AND appended to the (gitignored) history file
# so the stability record accumulates across runs.
total=$((pass + fail))
if [ "$fail" -eq 0 ]; then
    summary="smoke: $pass/$total PASS"
else
    summary="smoke: FAIL $first_fail — advisory ($pass/$total passed)"
fi
echo "$summary"
printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "$summary" >> "$SMOKE_DIR/.history"
if [ "$fail" -gt 0 ]; then exit 1; fi
exit 0
