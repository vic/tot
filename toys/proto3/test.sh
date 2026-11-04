#!/usr/bin/env bash
# Build + clippy (0 warnings), then every test isolated: 1 s timeout, 8 GiB address cap.
# Picks the TEST binary from cargo's JSON output (not "newest file") and FAILS on 0 tests.
set -u
cd "$(dirname "$0")"
OUT=$(timeout 300 systemd-run --user --scope -q -p MemoryMax=12G nice -n 10 nix-shell -p cargo rustc clippy --run \
  'cargo build --release 2>&1 | grep -E "^(warning|error)"; cargo clippy --all-targets --release --message-format=short 2>&1 | grep -E "^(src|warning|error)" ; echo "@@"; cargo test --release --no-run --message-format=json 2>/dev/null | grep -o "\"executable\":\"[^\"]*\"" | tail -1' 2>&1 | grep -v "search path")
LINT=$(printf '%s\n' "$OUT" | grep -E '^(src/|warning|error)')
T=$(printf '%s\n' "$OUT" | sed -n 's/.*"executable":"\(.*\)"/\1/p' | tail -1)
nwarn=$(printf '%s\n' "$LINT" | grep -c . )
[ "$nwarn" -gt 0 ] && printf 'clippy/rustc:\n%s\n' "$LINT"
[ -x "$T" ] || { echo "FAIL: no test binary"; exit 1; }
echo "test binary: $T"
pass=0; fail=0; slow=0
for t in $(bash -c "ulimit -v 8388608; $T --list 2>/dev/null" | sed -n 's/: test$//p'); do
  s=$(date +%s%N)
  bash -c "ulimit -v 8388608; exec timeout 1 nice -n 10 $T --exact $t -q" >/tmp/proto2-test.$$ 2>&1; rc=$?
  ms=$(( ($(date +%s%N) - s) / 1000000 ))
  if [ $rc = 0 ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL rc=$rc ${ms}ms $t"; grep -E "panicked|assert" /tmp/proto2-test.$$ | head -3; fi
  [ $ms -gt 1000 ] && slow=$((slow+1))
  echo "$ms $t" >> /tmp/proto2-times.$$
done
sort -rn /tmp/proto2-times.$$ 2>/dev/null | head -3 | sed 's/^/slowest ms: /'
rm -f /tmp/proto2-test.$$ /tmp/proto2-times.$$
echo "tests pass=$pass fail=$fail over1s=$slow lint=$nwarn"
[ $pass -gt 0 ] && [ $fail = 0 ] && [ $nwarn = 0 ]
