# pragmajs

Unified CLI for pragma-own and pragma-rt. Each file is parsed once with
`pragma-parse`, then both checkers run on that program.

```bash
cargo run -p pragmajs -- check crates/own/examples/
cargo run -p pragmajs -- check --runtime none --target ecmascript file.js
cargo run -p pragmajs -- build --target ecmascript crates/rt/fixtures/sqrt.js out.js
```

`pragma-own` and `pragma-rt` stay libraries. The own wasm playground still
builds with `cargo build -p pragma-own --lib`.
