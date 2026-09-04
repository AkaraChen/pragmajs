# pragma-rt

`pragmajs` 里的 refinement-type 检查器（原 refinejs）。

Flux-style refinement types for JavaScript. `pragmajs check` statically proves
liquid-type obligations with Z3; `pragmajs build` additionally preserves the
existing `__rt.assert` runtime checks in the emitted JavaScript. The `pragma-rt`
crate is the checker library; the CLI lives in `pragmajs`.

## Syntax

Use `/*#rt ... */` comments (deliberately incompatible with JSDoc's `/**`):

```js
/*#rt
 * type: (n: number | n > 0) => number | $ > 0
 */
function sqrt(n) {
  return Math.sqrt(n);
}

/*#rt type: number | x > 0 */
const x = 9;
```

`$` refers to the return value.

Predicate expressions support safe-integer literals, `+`, `-`, `*`, ordered
comparisons, equality, boolean values, `!`, `&&`, and `||`. Ordinary JS
`Number` arithmetic is solved as IEEE-754 binary64. Type indices, dense-array
lengths, and integer loop counters use a separate logical `int` sort; see
[Indexed types and liquid inference](docs/indexed-types-and-liquid-inference.md).
Division is deliberately outside the checked subset. Function parameter and
return refinements are checked both inside the function and at every call site.

Indexed types follow Flux: `number[10]` is the singleton `10`, and
`boolean[0 < n]` is the boolean whose value is the index formula. Names that
appear in an index are logical integers and must be safe integers at call
sites. Array literals introduce an opaque `DenseArray<T>[n]`; `push`/`pop`
update `n`, and `xs[i]` is allowed when `0 <= i < n` is proved. Ordinary
sparse `Array` indexing stays rejected. The flux-rs surface ports, the
rules used to accept or skip a Rust test, and the checker tradeoffs they
forced are in [flux-rs porting rules](docs/flux-rs-porting.md).

`while` and C-style `for` are checked. Loop-head invariants are inferred by
Houdini over scraped qualifiers (`0 <= v`, `v < length`, postcondition atoms).
A Z3 `unknown` result is a failure, not a proof.

## CLI

From the workspace root:

```bash
cargo run -p pragmajs -- check --target auto crates/rt/fixtures/sqrt.js
cargo run -p pragmajs -- build --target ecmascript crates/rt/fixtures/sqrt.js output.js
```

`--target` selects the refinement-aware standard prelude: `auto`,
`ecmascript`, `browser`, `node`, `deno`, or `bun`.

Compiler-backed library types (Corsa + tsconfig) are documented in
[Compiler-backed platform refinements](docs/compiler-backed-platform-refinements.md).

## Playground

Static snapshots under `playground/` (`pragmajs check --target ecmascript`).
Regenerate after checker or fixture changes:

```bash
cargo build -p pragmajs
node crates/rt/playground/generate.mjs
```

## License

MIT
