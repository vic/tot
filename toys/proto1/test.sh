#!/usr/bin/env bash
# Build, clippy, then every test isolated: 1 s timeout, 8 GiB address cap.
set -u
cd "$(dirname "$0")"
timeout 300 systemd-run --user --scope -q -p MemoryMax=12G nice -n 10 nix-shell -p cargo rustc clippy --run \
  'cargo clippy --all-targets --release 2>&1 | grep -E "^(warning|error)" | sort | uniq -c; cargo test --release --no-run 2>&1 | tail -1' 2>&1 | grep -v "search path"
T=""
# newest binary that lists tests (the non-test bin lists none)
for c in $(ls -t "${DEPS:-target/release/deps}"/proto1-* 2>/dev/null | grep -v '\.d$'); do
  if [ -x "$c" ] && bash -c "ulimit -v 8388608; $c --list 2>/dev/null" | grep -q ': test$'; then T=$c; break; fi
done
if [ -z "$T" ]; then echo "FAIL: zero tests found"; exit 1; fi
pass=0; fail=0; slow=0
for t in $(bash -c "ulimit -v 8388608; $T --list 2>/dev/null" | sed -n 's/: test$//p'); do
  s=$(date +%s%N)
  bash -c "ulimit -v 8388608; exec timeout 1 nice -n 10 $T --exact $t -q" >/dev/null 2>&1; rc=$?
  ms=$(( ($(date +%s%N) - s) / 1000000 ))
  if [ $rc = 0 ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL rc=$rc $t"; fi
  [ $ms -gt 1000 ] && slow=$((slow+1))
  echo "$ms $t" >> /dev/stderr
done 2> >(sort -rn | head -3 | sed 's/^/slowest ms: /')
echo "tests pass=$pass fail=$fail over1s=$slow"
