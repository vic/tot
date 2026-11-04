#!/usr/bin/env bash
# T14 GPU probe, fullscreen, vsync. keys: wasd/arrows move, q/e turn, +/- zoom, 1..5 horde 300/1k/3k/10k/30k, x quit.
# GPU_BENCH=1 ./run.sh -> gpu-bench.jsonl (uncapped present mode). perf per second -> gpu-perf.log
cd "$(dirname "$0")"
exec env LD_LIBRARY_PATH="$(cat .ldpath)" ./target/release/gpu "$@"
