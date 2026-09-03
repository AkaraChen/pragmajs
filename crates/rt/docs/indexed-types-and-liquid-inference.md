# Indexed types, opaque dense arrays, and loop-head inference

This note records the tradeoffs taken while adding Flux-style indexed types,
length-indexed dense arrays, and loop-head liquid inference. It is the
boundary for those features, not a changelog.

The surface ideas follow [flux-rs](https://github.com/flux-rs/flux) (indexed
singletons, length-indexed vectors, qualifier-based loop invariants). The
solver path stays in-process Z3 from the existing `z3` crate. Qualifier
inference at loop heads is a Houdini loop over scraped candidates rather than
a bundled `liquid-fixpoint` binary.

## Logical `int` versus JS `Number`

JS `Number` arithmetic is still IEEE-754 binary64, including rounding, NaN,
and infinity. That model is unchanged for ordinary `+`, `-`, `*`, and for
predicates that talk about JS numbers.

A separate logical integer sort is used for:

- type indices in `T[e]` (for example `number[10]` and `boolean[0 < n]`);
- `length` of a dense array;
- loop counters that start at a safe-integer literal and are updated by
  integer `+` / `-` / `+=` / `++`.

Logical integers are mathematical `int`, not binary64. Relating an integer
term `i` to a JS number uses an exact conversion on the safe-integer range
(`|n| <= 2^53 - 1`). That conversion is monotonic, and it commutes with
addition and multiplication inside that range. Outside it, integer reasoning
must not be trusted to match JS.

A function parameter of type `number` stays a JS number unless it appears in
an index expression (`boolean[0 < n]`, `DenseArray<T>[n]`). Index parameters
must be safe integers at every call site; a non-integer argument is a static
error, not a rounded index. A parameter index is a precondition (assumed in
the body, proved at calls). A return index is a postcondition (proved at
`return`, assumed by the caller). Predicates on those Int names (`n >= 0`)
use logical integer literals, not IEEE-754 `Number(0)`.

Division, non-integer literals, and `Math.*` stay in the IEEE-754 fragment.

## Opaque dense arrays versus real `Array`

Array literals introduce an opaque `DenseArray<T>[n]` whose length `n` is a
logical integer. Length changes only through the trusted mutation API:

- `push` proves nothing extra about elements, but sets length to `n + k`;
- `pop` requires `n > 0`, returns the last element (not `undefined`), and
  sets length to `n - 1`;
- element access `xs[i]` is allowed on dense arrays when `i` is a logical
  integer (literal or variable) and `0 <= i < n` is proved.

Ordinary sparse `Array` values keep the previous rule: indexing is rejected
because holes yield `undefined`. Compiler-backed `Array` types do not become
dense just because the printed name matches.

Dense arrays are treated like Flux's opaque `RVec`: clients do not invent
length facts from object shape. Alias-aware heap updates of `.length` are
out of scope; trusted methods update the length index of the receiver term.

## What loop inference will and will not infer

`while` and C-style `for` are in the checked subset. `for-in` and `for-of`
are not. Compound `+=` / `-=` and `++` / `--` are accepted on tracked
numeric or integer bindings so that idiomatic `for` updates type-check.

At a loop head the checker does not unroll. It infers a loop invariant by
Houdini:

1. Collect variables assigned in the body (and the `for` update).
2. Scrape qualifier candidates from the loop test, from annotations in
   scope (including the current function's postcondition, with `$` rewritten
   to those variables), and from a small grammar: `0 <= v`, `v < n`,
   `v <= n`, `v == n`, `1 <= v`, and `v == i - lo` (assigned integer equal to
   another assigned integer minus an unmodified integer) over integer/number
   terms in the test and the modified set.
3. Start from the full candidate set. Havoc the modified variables **and**
   heap/length facts, assume the remaining qualifiers, and drop any qualifier
   that is not implied by loop entry or not preserved by every path through
   the body (including the `for` update). A Z3 `unknown` result drops that
   qualifier; it is never a proof.
4. After the loop, assume the surviving invariant and the negated test.
   The exit state is the havoc'd head, not the body's post-state, so a
   `push`/`pop` inside the body does not leave a precise length afterward.

This is enough for a factorial-style numeric postcondition, a scrape-style
`res == hi - lo` range count, and a dense array walk that indexes with
`i < length`, without writing the invariant by hand. A walk can use `i < xs.length` from the loop test even though the
concrete pre-loop length is forgotten. It will not infer heap-field
predicates, quantified element facts, or a length kvar: `while (xs.length > 0)
xs.pop()` is safe *inside* the loop because of the test, but a following
`xs.pop()` or `xs[i]` is rejected. If Houdini weakens to `true` and the
postcondition still needs a loop fact, the obligation fails closed.

How those rules show up in the flux-rs ports (empty `[]`, `min_index` post
weakened to `$ >= 0`, call-site `DenseArray[n]`, Int `===` for hex
literals) is recorded in [flux-rs porting rules](flux-rs-porting.md).

## Copied or adapted from flux-rs

- Indexed singletons and bool-as-index (`bool[0 < n]`) follow Flux's
  surface meaning, not its rustc encoding.
- Length-indexed dense arrays follow the opaque `RVec` API (`push`/`pop`/
  `get` with `i < n`), not Rust ownership or `&mut` `ensures`.
- Loop-head unknown refinements follow liquid typing's qualifier/Houdini
  idea (see Flux's loop-invariant inference). Candidate scraping is local
  to this crate; flux-rs source is not vendored, and `liquid-fixpoint` is
  not a runtime dependency.
