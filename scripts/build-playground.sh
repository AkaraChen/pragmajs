#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release --lib --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir playground/pkg \
  target/wasm32-unknown-unknown/release/ownershipjs.wasm
# Keep the glue import path that app.js uses.
ls -lh playground/pkg
