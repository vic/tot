#!/usr/bin/env bash
# Build (capped, no flakes) if needed, then play in THIS terminal (Ghostty >= 1.4.0, 120x40+).
# ./play.sh [--seed N]            play; telemetry -> results/play-<unix>.jsonl
# GHOSTTY=/path/to/ghostty ./play.sh --window   same, in a clean fullscreen Ghostty window
set -u
P=$(cd "$(dirname "$0")" && pwd)
B=$P/target/release/tot
if [ ! -x "$B" ] || [ -n "$(find "$P/src" "$P/Cargo.toml" -newer "$B" 2>/dev/null | head -1)" ]; then
  timeout 300 systemd-run --user --scope -q -p MemoryMax=12G nice -n 10 \
    nix-shell -p cargo rustc --run "cd '$P' && cargo build --release 2>&1 | tail -2" || exit 1
fi
if [ "${1:-}" = "--window" ]; then
  shift
  exec "${GHOSTTY:-ghostty}" --config-default-files=false --gtk-single-instance=false \
    --quit-after-last-window-closed=true --fullscreen=true --font-size=10 \
    -e bash -c "'$B' play $* ; echo; read -rp 'press enter to close'"
fi
exec "$B" play "$@"
