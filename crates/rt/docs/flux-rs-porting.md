# flux-rs porting rules and tradeoffs

This note records how refinejs ports [flux-rs](https://github.com/flux-rs/flux)
surface tests into JavaScript, and the checker tradeoffs those ports forced.
It is the rulebook, not a file-by-file skip list. The skip list lives in
[flux-rs test triage](flux-rs-test-triage.md). The sort/array/loop model lives
in [Indexed types and liquid inference](indexed-types-and-liquid-inference.md).

JS fixtures are `fixtures/flux_*.js`, driven by `tests/flux.rs`. The same
files are the playground catalog (`playground/catalog.json`).

## Porting rules

A flux-rs file is ported only when there is an **honest JS analogue** inside
refinejs's checked subset. Honest means:

1. The JS program has the same refinement *claim* as the Rust fragment that
   survived the translation, not a weaker claim dressed up as the original.
2. Rust-only surface is dropped, not faked: `&mut` / `strg`, structs, enums,
   traits, const generics, macros, overflow attributes, bitvectors, raw
   pointers, and `liquid-fixpoint` kvars on heap fields.
3. Polarity is preserved. A positive fixture must statically verify **and**
   run under Node with `__rt.assert` still in the transpiled output. A
   negative fixture must report a **definite** refinement error — a Z3
   `unknown` is not a rejection.
4. One flux-rs file may become several JS fixtures when it mixes a portable
   operator with a non-portable one (`binop.rs` logical vs bitwise;
   `operators.rs` `!`/`-`/`!==` vs `/`).
5. Adapt rather than invent. If the Rust test depends on `toss()`,
   `i32::MAX`, a struct field, or an if-expression in an index, drop that
   part and keep the obligation that still makes sense in JS. Record the
   adaptation in a leading comment on the fixture.
6. Do not copy flux-rs diagnostic wording. refinejs errors stay refinejs
   errors.
7. Do not vendor flux-rs or depend on a `liquid-fixpoint` binary.

Everything else is skipped with a reason in the triage note. The deferred
portable list is empty: each remaining surface file is either ported or
skipped.

## Semantic mapping

| flux-rs | refinejs |
|---|---|
| `i32` / `int` used as an index | `number[n]` with logical `int` sort |
| `bool[b]` | `boolean[b]` |
| `RVec<T>[n]` | opaque `DenseArray<T>[n]` |
| `rvec![]` / `RVec::new()` | not `[]` (that is `DenseArray<unknown>`); seed `[0]` then `pop` for a typed empty |
| `v.push` / `v.get(i)` | `v.push` / `v[i]` with `0 <= i < n` |
| `while` / C-style `for` | same; `for-in` / `for-of` stay rejected |
| if-expression in an index | a predicate (`($ === a \|\| $ === b) && $ <= a && $ <= b`); the subset has no `?:` |
| bitwise `&` / `\|` / shifts | skipped; logical `!` / `\|\|` on bool indexes are ported |
| hex / octal / binary in specs | `0x` / `0o` / `0b` in the predicate lexer |
| `&mut self` / `ensures` | dropped; mutation is the trusted `push`/`pop` length update |
| named `const` in signatures | skipped |
| `/` and `%` | skipped (division is outside the checked subset) |

Index parameters are preconditions (assumed in the body, proved at calls).
Return indexes are postconditions (proved at `return`, assumed by the caller).

## Tradeoffs the ports forced

These are the places where “just translate the Rust” is the wrong move.

### Dual sort: logical `int` versus IEEE `Number`

JS `Number` arithmetic stays IEEE-754 binary64. Type indexes, dense-array
lengths, and integer loop counters use a separate mathematical `int` sort.
A parameter of type `number` stays a JS number unless it appears in an
index (`boolean[0 < n]`, `DenseArray<T>[n]`, `number[n]`). Those names are
`Int` at function entry; call sites must pass safe integers.

Predicates on Int names (`n >= 0`, `$ === n + 0xa`) must emit integer
literals, not `Number(0)`. Equality `===` uses the same inferred numeric
sort when both sides are Int. Mixing the sorts through IEEE equality made
countdown, identity returns, and hex-literal specs unprovable.

Relating `int` to `Number` is an exact conversion on the safe-integer
range. Outside that range the integer reasoning is not trusted to match JS.

### Call-site argument qualifiers

An array literal such as `[1, 2]` carries `length === 2` on the value's
qualifier. Proving a parameter `DenseArray<number>[n]` at a call
`denseLen(2, [1, 2])` needs that fact in `state.assumptions`, not only on
the value. Without the push, `minIndex(3, [3, 1, 2])` and `denseLen(2, [1, 2])`
failed with “Argument 2 does not match its index” even though the length
was obvious.

### Opaque `DenseArray` versus sparse `Array`

Array literals introduce `DenseArray<T>[n]`. Length changes only through
`push` / `pop`. Indexing is allowed when `i` is a logical integer and
`0 <= i < n` is proved. Ordinary sparse `Array` indexing stays rejected
because holes yield `undefined`. Compiler-backed `Array` types do not
become dense because the printed name matches.

Bare `[]` is `DenseArray<unknown>`. Flux's `rvec![]` then `push` is not
`const v = []; v.push(1)`. The port seeds `[0]`, pops, then pushes so the
element type is `number`.

### Loop-head Houdini, not `liquid-fixpoint`

Loop-head invariants are a Houdini loop over scraped candidates (`0 <= v`,
`v < length`, `v == n`, `v == i - lo`, postcondition atoms). A Z3
`unknown` drops the qualifier; it is never a proof. flux-rs source is not
vendored and `liquid-fixpoint` is not a runtime dependency.

Candidate scrape is local. Heap-field kvars are out of scope, so
`scrape01` (loop push that needs a length kvar) is skipped rather than
half-ported.

### Fail-closed heap havoc at loop heads

Havoc of assigned scalars is not enough. A length snapshot `sz = xs.length`
or a concrete pre-loop length is a heap fact. After
`while (xs.length > 0) xs.pop()`, a following `xs.pop()` must be rejected.
The checker therefore invalidates heap facts at every loop head, not only
the assigned scalars.

Consequence: a dense walk cannot keep `sz === xs.length` across the loop.
Write `i < xs.length` in the test. The postcondition on the index of the
minimum is `$ >= 0`, not `$ < 3`, because the concrete length `3` is
forgotten at the head. That is weaker than Flux's `min_index` on a
fixed-length `RVec`, and it is the sound JS claim.

Loop *exit* is the havoc'd head plus the negated test, not the body's
post-state. A `push` inside the body does not leave a precise length
afterward.

### No if-expressions in indexes

Flux writes `if a < b { a } else { b }` as an index. refinejs has no
`ConditionalExpression` in the subset. `min` becomes a predicate on `$`.

### Logical operators only

Rust `bool` bitwise `|` is JS `||` in the port. Bitwise and shift functions
in `binop.rs` stay skipped (integer overflow / bitvector territory).

### Solver policy after ablation

Production checks the preprocessed implication directly with SMT, and only SMT
`Unsat` is a proof. The former Fixedpoint-first policy remains selectable as a
legacy differential backend. Binding identity (`Same`) uses SMT equality; JS
`===` on `Number` uses IEEE `eq_fpa`.

### Ownership is not welded on

A sibling `ownershipjs` experiment is not part of this port. JS analogue
of `&mut` / `strg` / packed structs is “don't port that file”.

## Ported fixtures

| flux-rs | JS | Adaptation |
|---|---|---|
| `pos/surface/index00.rs` | `flux_assert_index_{positive,negative}.js` | `boolean[true]` assert + singleton `five`/`incr` |
| `pos/surface/test00.rs`, `test05.rs` | `flux_inc_dec_positive.js` | inc/dec pre-post |
| `neg/surface/test00.rs` | `flux_inc_dec_negative.js` | inc claiming `$ < x` |
| `pos/surface/test06.rs` | `flux_double_positive.js` | `x + x` with `0 < x` |
| `pos/surface/rvec00.rs` | `flux_rvec_literal_positive.js` | empty length 0 + `[0, 1]` index; no `rvec![e; n]` |
| `neg/surface/rvec00.rs` | `flux_rvec_oob_negative.js` | `v[2]` on length 2 |
| `pos/surface/test02.rs` | `flux_rvec_push_get_positive.js` | seed `[0]`/`pop`/`push`; return `v.length` as `number[2]` (no element-value facts). Dropped `&mut` |
| `neg/surface/test02.rs` | `flux_rvec_push_get_negative.js` | get past last push |
| `pos/surface/fib_loop.rs` | `flux_fib_loop_{positive,negative}.js` | `while k > 2` |
| `pos/surface/loop01.rs` | `flux_loop01_{positive,negative}.js` | count-up Houdini `0 <= res` |
| `pos/surface/loop00.rs` | `flux_countdown_{positive,negative}.js` | dropped `toss()` / `i32::MAX`; countdown to `number[0]` |
| `pos/surface/scrape00.rs` | `flux_scrape_range_{positive,negative}.js` | `res == hi - lo` via `v == i - lo`; negative uses a wrong post (`hi-lo+1`) instead of disabling scrape |
| `pos/surface/if-then-else.rs` | `flux_min_{positive,negative}.js` | if-index became a predicate |
| `pos/surface/arg_syntax.rs` | `flux_arg_path_positive.js`, `flux_exists_bound_positive.js` | `path` + `exists` only; skipped `&` / `[T; N]` / slice |
| `pos/surface/binop.rs` | `flux_logical_not_{positive,negative}.js`, `flux_logical_or_index_positive.js` | logical `!` / `\|\|`; bitwise and `/` skipped |
| `pos/surface/operators.rs` | `flux_unary_neg_*`, `flux_neq_*`, `flux_not_pred_*`, `flux_bool_not_index_*` | `-x`, `!==`, `!(x > 0)`, `boolean[!x]`; `/` skipped |
| `pos/surface/literals00.rs` | `flux_literals_hex_positive.js` | `0xa` / `0o12` / `0b1010` in specs |
| `pos/surface/ex2_min_index_loop.rs` | `flux_min_index_{positive,negative}.js`, `flux_dense_param_{positive,negative}.js` | dropped struct `Bob`; walk uses `i < xs.length`; post `$ >= 0` |

Native refinejs cases that are *not* flux-rs ports (indexed singletons,
dense walk/push/pop, factorial, polymorphism, typestate, subset rejections)
stay in `fixtures/flux_*.js` and appear in the playground under origin
“refinejs”. They are not counted as flux-rs coverage.

## What this package is not

- Not a rustc/Flux clone. Surface meaning of indexes and opaque vectors,
  not the rustc encoding.
- Not ownership, packed structs, or `&mut ensures`.
- Not IEEE-754 replaced by mathematical integers. Both sorts exist.
- Not a general heap-field kvar solver. Loop-push length inference
  (`scrape01`) is skipped on purpose.
- Not bitvector / overflow / const-generic / trait / enum coverage.

Coverage of the flux-rs surface corpus (ported or skipped with a reason)
is 320/320 pos and 189/189 neg. That is a classification claim, not “refinejs
proves every Flux test”.
