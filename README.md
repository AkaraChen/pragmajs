# ownershipjs

Static ownership and borrow checker for JavaScript and TypeScript.

It parses `.js` / `.ts` with [oxc](https://oxc.rs), reads `/*#own ... */`
comments, and reports use-after-move, double-move, mutable-borrow conflicts,
and lifetime errors. There is **no runtime**: nothing is generated, injected,
or rewritten.

The checker follows Austral’s linearity algorithm
([how it works](https://borretti.me/article/how-australs-linear-type-checker-works)).
Full syntax, state table, and soundness limits: [`docs/design.md`](docs/design.md).

## Install

```bash
cargo install --path .
```

Or run from the repo:

```bash
cargo run -- --check examples/
cargo test
```

## Usage

```bash
ownershipjs --check path/to/file.js
ownershipjs --check examples/
```

Exit `0` if there are no diagnostics, `1` if any ownership/borrow error was
reported. Output looks like:

```
examples/err-unique-forget.js:11:18: error[unique-forget]: unique value `buf` is not consumed
```

## Annotations

Only functions with a `/*#own type: ... */` comment are checked.

```js
/*#own
 * type: (buf: unique Buffer) => void
 */
function process(buf) {
  consume(buf);
}
```

### Types

| Form | Rule |
| --- | --- |
| `unique T` | Consume **exactly once** (linear). Forgetting or using after move is an error. |
| `affine T` | Consume **at most once**. Silent drop at end of scope is allowed. |
| `copy T` | Unrestricted reuse. |
| `&readonly T` | Immutable borrow. |
| `&mut T` | Mutable borrow. Exclusive: no overlapping `&mut`, no mix with `&readonly` in the same expression. |
| `void` | No owned return. |

Passing a `unique` / `affine` value to a matching parameter **moves** it.
`void buf` also consumes. `buf.field` is a path read and does not consume.

A callee whose parameter is `&readonly T` or `&mut T` borrows that argument
for the call only.

### Locals, lexical borrows, clone

```js
/*#own let x: unique Buffer */
const x = make();

/*#own borrow buf as view: &readonly Buffer */
const view = buf;          // borrow, not a move; lives until the end of the block

/*#own clone buf as copy */
const copy = buf;          // duplicate; `buf` is still unconsumed

read(/*#own &readonly */ buf);
write(/*#own &mut */ buf);
```

A borrow must not be returned or assigned out of its block (`borrow-escape`).
The owner cannot be consumed while borrowed.

## Examples

`examples/ok-*` should check clean. `examples/err-*` should report the named
rule.

| File | What it shows |
| --- | --- |
| `ok-unique-move.js` / `err-unique-forget.js` / `err-unique-use-after-move.js` / `err-unique-double-move.js` | unique move |
| `ok-affine-drop.js` / `err-affine-use-after-move.js` | affine drop vs use-after-move |
| `ok-readonly-borrow.js` / `err-consume-while-borrowed.js` | `&readonly` |
| `ok-mut-borrow.js` / `err-overlapping-mut.js` / `err-readonly-mut-conflict.js` | `&mut` |
| `ok-lifetime-scope.js` / `err-borrow-escape.js` | lexical region |
| `ok-copy.js` / `ok-copy.ts` / `ok-clone.js` | Copy / Clone |
| `ok-branch-consume.js` / `err-branch-inconsistent.js` / `err-consume-in-loop.js` | Austral branch and loop rules |

```bash
cargo run -- --check examples/ok-unique-move.js   # exit 0
cargo run -- --check examples/err-unique-forget.js  # error[unique-forget]
```

## Limits

Single-file. Cross-file calls match by function name when the callee is
annotated in the same file.

These JS constructs are **unmapped** (reported, not silently accepted):
`eval`, `with`, computed keys on owned values, prototype / `__proto__`
mutation, nested functions that capture owned bindings.

See [`docs/design.md`](docs/design.md) for the algorithm and soundness
assumptions.

## License

MIT
