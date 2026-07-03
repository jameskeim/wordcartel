#!/bin/sh
# clean-stale.sh — sweep abandoned wcartel-smoke-* tmux servers left by
# crashed/interrupted runs. Sweeps ONLY sockets matching the smoke glob in the
# current user's tmux socket dir — never the user's own tmux servers.
set -u
dir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
if [ ! -d "$dir" ]; then
    echo "clean-stale: no tmux socket dir ($dir) — nothing to sweep"
    exit 0
fi
n=0
for sock in "$dir"/wcartel-smoke-*; do
    [ -e "$sock" ] || continue
    name=$(basename -- "$sock")
    tmux -L "$name" kill-server 2>/dev/null || true
    rm -f "$sock"
    n=$((n + 1))
    echo "clean-stale: swept $name"
done
echo "clean-stale: $n socket(s) swept"
