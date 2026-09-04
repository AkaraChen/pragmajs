# pragmajs

Unified CLI for pragma-own and pragma-rt. Each file is parsed once with
`pragma-parse`, then both checkers run on that program.

```bash
cargo run -p pragmajs -- check crates/own/examples/
cargo run -p pragmajs -- check --runtime none --target ecmascript file.js
cargo run -p pragmajs -- build --target ecmascript crates/rt/fixtures/sqrt.js out.js
```

Corsa is on by default. The TypeScript 7 native compiler (`tsgo`) is downloaded
once into `~/.cache/pragmajs/` if `CORSA_BIN` / `TSGO` / PATH do not already
provide it. Pass `--no-corsa` to skip TypeScript.

`pragma-own` and `pragma-rt` stay libraries. The own wasm playground still
builds with `cargo build -p pragma-own --lib`.
