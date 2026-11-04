#!/usr/bin/env bash
# proto1 on Ghostty: 3 runs of `suite all`, clean config, fullscreen, font-size 8 (=235x61 cells on
# 1920x1080 @1.25 niri; proto draws a 200x60 region). Results: results/ghostty-<rev>.jsonl.
# Usage: ./run-all.sh [ghostty-binary]   (default: ghostty on PATH)
set -u
P=$(cd "$(dirname "$0")" && pwd)
G=${1:-ghostty}
REV=$("$G" +version | sed -n 's/.*- version: //p')
OUT="$P/results/ghostty-$REV.jsonl"
GFLAGS=(--config-default-files=false --gtk-single-instance=false --quit-after-last-window-closed=true
  --fullscreen=true --font-size=8)
for run in 1 2 3; do
  # 12 scenes x 5 s + probes ~= 70 s; cap 150 s. ghostty is this shell's child: timeout kills only it.
  bash -c "ulimit -v 8388608; exec timeout 150 nice -n 10 \"$G\" ${GFLAGS[*]} -e bash -c \
    '$P/target/release/proto1 suite all --cols 200 --rows 60 --secs 5 --run $run --out $OUT 2>>$P/results/ghostty-$REV.err'"
  echo "run $run exit=$?"
done
"$P/target/release/proto1" analyze "$OUT"

# K1 + L1 need a human (vic): run inside the same clean Ghostty, then press/hold/release
# w a s d Esc Up Down Left Right (30 s max). Appends K1/L1 lines to the same jsonl:
#   ghostty --config-default-files=false --gtk-single-instance=false --font-size=8 \
#     -e bash -c "$P/target/release/proto1 keys --secs 30 --out $OUT; read -p done"
