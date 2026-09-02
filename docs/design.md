# ownershipjs design

A comment-driven static checker for JavaScript and TypeScript ownership and
borrows. The implementation is a Rust CLI that **parses** source with [oxc](https://oxc.rs)
and **never generates, injects, or rewrites** JS/TS. There is no runtime.

The algorithm is Austral’s published linearity checker
([Borretti, “How Austral’s Linear Type Checker Works”](https://borretti.me/article/how-australs-linear-type-checker-works);
`austral/lib/LinearityCheck.ml`). The Rust port in
[`lambdaclass/austral.rs`](https://github.com/lambdaclass/austral.rs)
(`linearity_check.rs`) is **incomplete** (borrow is still `TODO`); this crate
follows the OCaml algorithm, not that unfinished file.

## `/*#own` annotation syntax

All ownership information lives in block comments that start with `#own`.
oxc attaches leading comments to the next token (`Comment.attached_to`).

### Function signatures

```js
/*#own
 * type: (buf: unique Buffer) => void
 */
function process(buf) { ... }
```

Parameter and return types:

| Form | Meaning |
| --- | --- |
| `unique T` | Linear: consume **exactly once** on every path (Austral `Linear`). |
| `affine T` | Affine: consume **at most once**; silent drop at end of scope is allowed. |
| `copy T` | Unrestricted / Free (Austral `Free`). |
| `&readonly T` | Immutable borrow (Austral `&[T, R]`). |
| `&mut T` | Mutable borrow (Austral `&![T, R]`). |
| `void` / `Unit` | No owned return. |

A bare type name `T` is treated as `unique T`.

### Locals, statement borrows, clone, drop

The OBJECTIVE only showed function-level `type:`. Locals, lexical borrows,
and Copy/Clone cannot be named there, so the comment language is extended
**inside the same `/*#own` tag** (not TS types, not a different tag):

```js
/*#own let x: unique Buffer */
const x = make();

/*#own borrow buf as view: &readonly Buffer */
const view = buf;          // borrow, not a move

/*#own borrow! buf as mutv: &mut Buffer */
const mutv = buf;

/*#own clone buf as copy */
const copy = buf;          // duplicate; `buf` stays unconsumed

/*#own drop buf */         // explicit consume (destructor)
```

### Expression-level shorthand (Austral `&x` / `&!x`)

```js
read(/*#own &readonly */ buf);
write(/*#own &mut */ buf);
```

A callee whose `type:` parameter is `&readonly T` or `&mut T` counts as a
shorthand borrow of that argument (no extra comment needed).

Passing a `unique`/`affine` argument to a `unique`/`affine` parameter is a
**move**. `void buf` (or any other value-position use) is also a consume.

Member access `buf.field` is a **path** (Austral Rule 6): it does not consume.

## Borrow-check algorithm

Per annotated function:

1. Build a **state table** of linear-ish bindings. Each row is
   `(name, kind, loop_depth_at_definition, state)`.
2. `state` is one of **Unconsumed**, **BorrowedRead**, **BorrowedWrite**,
   **Consumed**. New unique/affine bindings start Unconsumed.
3. Traverse statements in execution order. For each expression, **count
   appearances** of every table name as:
   - consumed
   - borrowed immutably (`read`)
   - borrowed mutably (`write`)
   - path head
4. Apply Austral’s decision table (counts partitioned as Zero / One /
   MoreThanOne):

| State | Consumed | Write | Read | Path | Result |
| --- | --- | --- | --- | --- | --- |
| Unconsumed | 0 | 0 | — | — | OK |
| Unconsumed | 0 | 1 | 0 | 0 | OK (ephemeral `&mut`) |
| Unconsumed | 0 | 1 | ≠0 or path | | `mut-borrow-conflict` |
| Unconsumed | 0 | >1 | — | — | `mut-borrow-conflict` |
| Unconsumed | 1 | 0 | 0 | 0 | consume if loop depth matches |
| Unconsumed | 1 | other | | | `double-move` |
| Unconsumed | >1 | — | — | — | `double-move` |
| BorrowedRead | 0 | 0 | 0 | — | OK |
| BorrowedRead | other | | | | `consume-while-borrowed` / conflict |
| BorrowedWrite | 0 | 0 | 0 | 0 | OK |
| BorrowedWrite | other | | | | `consume-while-borrowed` / conflict |
| Consumed | 0 | 0 | 0 | 0 | OK |
| Consumed | other | | | | `use-after-move` or `borrow-after-move` |

5. **If / switch**: run branches independently; resulting states of outer
   names must agree (`branch-inconsistent` otherwise).
6. **Loops**: increment loop depth. Consuming a name whose definition depth
   differs is `consume-in-loop`. Creating and consuming inside the loop is OK.
7. **`borrow` statement**: owner must be Unconsumed (or already
   BorrowedRead for nested `&readonly`). Owner becomes BorrowedRead /
   BorrowedWrite for the **lexical block** that contains the binding. Nested
   `&mut` of an already mutably borrowed owner is `mut-borrow-conflict`.
   After the block, the alias is removed and the owner returns to Unconsumed
   when its borrow count hits zero.
8. **Return / throw**: remaining `unique` names must be Consumed
   (`unique-forget`). Affine may remain (implicit drop). Returning a borrow
   alias is `borrow-escape`.
9. **Scope end**: unconsumed `unique` → `unique-forget`; unconsumed `affine`
   is dropped; borrow aliases unborrow their owner.

Copy bindings are not entered in the table. Clone inserts a **new** unique/
affine row without consuming the source.

## Mapping to austral.rs / Austral

| Austral | ownershipjs |
| --- | --- |
| `Linear` universe | `unique T` |
| Affine (spec; destructor at block end) | `affine T` |
| `Free` | `copy T` |
| `Unconsumed` / `BorrowedRead` / `BorrowedWrite` / `Consumed` | same four states |
| Appearance counts `consumed, read, write, path` | same four counters |
| `consume_once` requires matching loop depth | `consume-in-loop` |
| `if` / `case` table consistency | `if` / `switch` |
| `borrow` / `borrow!` statement + region `R` | `/*#own borrow` / `borrow!` + JS block |
| Shorthand `&x` / `&!x` | `/*#own &readonly` / `&mut` or callee `&` param |
| Unspeakable region (no escape) | borrow alias cannot be returned or assigned out (`borrow-escape`) |
| `austral.rs` `linearity_check.rs` | **not** the spec; borrow still TODO there |

Examples under `examples/` are JS/TS adaptations of Austral
`006-linearity` / `007-borrowing` ideas (forget, double consume, branch
agreement, loop depth, `&` / `&mut`, region lifetime, Free/Clone).

## Runtime builtin prelude

Each check loads a **prelude** of `/*#own`-compatible signatures for that
runtime’s builtins. The user file does not re-annotate `console.log`,
`fs.readFile`, `Buffer.from`, `Deno.readFile`, `Bun.file`, and the rest of
the high-use set.

Selector (CLI `--runtime` / `-r`, library [`Runtime`](../src/prelude.rs)):

| Value | What is loaded |
| --- | --- |
| `node` (default) | Node builtins from `preludes/node.own` (inventory: `@types/node`) |
| `bun` | Node prelude **plus** Bun-only names from `preludes/bun.own` (`bun-types`) |
| `deno` | Deno namespace + shared web globals from `preludes/deno.own` (`lib.deno.ns.d.ts`) |
| `none` | Empty. Builtins behave as unknown callees. |

Ownership kinds are assigned by heuristic (the `.d.ts` packages have no
`unique` / `affine` / `&mut`). The inventory of distinct callables — module
functions **and** instance methods — is taken from Corsa/tsgo via
[corsa-bind](https://github.com/ubugeeei-prod/corsa-bind) (`scripts/gen-prelude.cjs`).
Overloads of the same JS name collapse to one row. Names Corsa cannot express
as an identifier callee (construct signatures, symbol members) go in the
stop-report, not the prelude.

Callee lookup is the dotted member path (`fs.readFile`, `Buffer.from`,
`Deno.readFile`, `Bun.file`) **or**, for instance calls `buf.toString()`,
`{ReceiverType}#{method}` using the type stored on the `/*#own` binding
(`Buffer#toString`, `FileHandle#close`). If that type is missing, the checker
tries a Corsa/tsgo binary (`TSGO` / `CORSA_BIN`); without a program, Corsa
cannot recover a stdlib type for a bare identifier, so those calls stay
unknown. For the Node set, last-segment aliases of `fs.*` (except
`fs.promises.*`) are also registered (`readFile` → `fs.readFile`).
File-level `/*#own type:` signatures overwrite the prelude.

A prelude `copy` or `&readonly`/`&mut` parameter does **not** consume a
unique/affine owner. A `unique`/`affine` parameter still moves. Extra
arguments on a known copy-style callee (e.g. `console.log(a, b)`) inherit
the last copy/ref mode and do not consume.

Switching runtimes changes which names exist: `Bun.file` is absent from
`node`; `Deno.readFile` is absent from `node` and `bun`. With `none`,
`console.log(buf)` is an unknown call and consumes `buf`.

## Soundness assumptions and limits

Single-file static checking. Cross-file calls match **by function name**
when the callee is annotated in the same file. Unknown callees (including
builtins when `--runtime none`) treat identifier arguments as consumes
(conservative). Prelude names are known callees for the selected runtime.

The following JS constructs are **not mapped** from Austral linearity and
are reported as `unmapped` rather than silently accepted:

- `eval`
- `with`
- computed property access `owned[expr]`
- prototype / `__proto__` mutation of owned values
- nested functions that capture owned bindings
- `switch` fall-through (cases are treated as disjoint, like Austral `case`)

Dynamic property names, the prototype chain, and runtime `this` aliasing
can smuggle a moved value in ways this checker cannot see. The checker is
sound **only** for code that stays within the annotated, identifier-based
fragment described above.

Not in scope: JS runtime wrappers, codegen, a full JS typechecker, Austral
typeclasses / capabilities / MLIR, persistence, network, or concurrency
scheduling.
