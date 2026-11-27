#!/bin/sh
# Build the static site into web/dist: tot as wasm32-wasip1 (tot.wasm), then vite.
set -e
cd "$(dirname "$0")"
timeout 300 nix build --impure -f tot-wasm.nix -o /tmp/tot-wasm
cp -L /tmp/tot-wasm/bin/tot.wasm tot.wasm
chmod u+w tot.wasm
npm install
npx vite build
