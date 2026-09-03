#!/bin/sh
set -eu
crate=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$crate/../.." && pwd)
cd "$root"
cargo build --release -p pragma-own --lib --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir "$crate/playground/pkg" --out-name ownershipjs \
  target/wasm32-unknown-unknown/release/pragma_own.wasm
# Keep the glue import path that app.js uses (`./pkg/ownershipjs.js`).
ls -lh "$crate/playground/pkg"
