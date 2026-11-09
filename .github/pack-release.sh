#!/usr/bin/env bash
# pack-release.sh <static-tot out path> <system> <dir>: tot-<system>.tar.gz in <dir>, holding `tot` (a relocatable
# launcher), libexec/tot (the static binary) and data/ (the binary has no data of its own).
set -eu
pkg=$1 system=$2 out=$3
root=$out/tot-$system
rm -rf "$root"
mkdir -p "$root/libexec"
cp "$pkg/libexec/tot" "$root/libexec/tot"
cp -r "$pkg/share/tot/data" "$root/data"
chmod -R u+w "$root"
cat > "$root/tot" <<'LAUNCHER'
#!/bin/sh
d=$(dirname "$(readlink -f "$0")")
exec "$d/libexec/tot" "$@" --data "$d/data" --out "${XDG_STATE_HOME:-$HOME/.local/state}/tot"
LAUNCHER
chmod +x "$root/tot"
tar -C "$out" -czf "$out/tot-$system.tar.gz" "tot-$system"
