# pragmajs

Unified CLI for pragma-own and pragma-rt. Each file is parsed once with
`pragma-parse`, then both checkers run on that program.

```bash
cargo run -p pragmajs -- check crates/own/examples/
cargo run -p pragmajs -- check --checker own crates/own/examples/
cargo run -p pragmajs -- check --checker rt crates/rt/fixtures/
cargo run -p pragmajs -- check --runtime none --target ecmascript file.js
cargo run -p pragmajs -- build --target ecmascript crates/rt/fixtures/sqrt.js out.js
```

Corsa is on by default. The TypeScript 7 native compiler (`tsgo`) is downloaded
once into `~/.cache/pragmajs/` if `CORSA_BIN` / `TSGO` / PATH do not already
provide it. Pass `--no-corsa` to skip TypeScript.

Use `--checker all|own|rt` to choose the combined checker (the default), the
ownership checker only, or the refinement checker only. Unselected checkers and
their annotation parsers are not run.

In `--checker own` mode, automatic Corsa discovery is skipped unless an
ownership annotation omits a payload type. Explicit `--corsa` or `--tsconfig`
options still force the compiler gate. An own-only build emits transpiled
JavaScript without the refinement runtime.

`pragma-own` and `pragma-rt` stay libraries. The own wasm playground still
builds with `cargo build -p pragma-own --lib`.
