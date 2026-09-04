# Compiler-backed platform refinements

Status: implemented on `master` in `e04695e` and documented on 2026-09-02.

This document records the compiler-backed standard-library work, the choices
made while implementing it, the soundness boundaries enforced by refinejs,
and the evidence used to validate the result. It is a snapshot of the shipped
implementation rather than a roadmap.

## Outcome

refinejs now combines two sources of information:

1. A curated, refinement-aware catalog describes semantic facts that an
   ordinary TypeScript declaration cannot express, such as Array result length,
   callback provenance, receiver mutation, and callback timing.
2. An optional Corsa-backed TypeScript project supplies ordinary type and symbol
   information for APIs outside that curated catalog.

The compiler is a type-information provider, not a refinement prover. Z3 and
the refinejs verifier remain responsible for proving every refinement
obligation. Compiler evidence is accepted only after project-wide diagnostics,
configuration, declaration provenance, suppression directives, local
implementations, and unknown effects have passed explicit trust gates.

This split lets refinejs understand selected ECMAScript, browser, Node, Deno,
and Bun semantics without maintaining handwritten signatures for every library
member, while avoiding the unsound assumption that every value with a familiar
printed type name is a trusted standard-library value.

## End-to-end checking flow

For ordinary checking, refinejs parses annotations, selects the requested
target catalog, and verifies the program directly.

Compiler-backed checking adds the following stages:

1. Canonicalize the source file, Corsa executable, and `tsconfig` paths.
2. Resolve the TypeScript project through Corsa and require the checked source
   to be the exact on-disk file included in that project.
3. Read the complete program file set and compiler options.
4. Reject unsafe compiler settings and diagnostic-suppression directives in
   implementation sources. Declaration files are treated as explicit trust
   roots.
5. Collect diagnostics for the whole configured project. Any compiler error
   prevents refinement verification.
6. Parse the JavaScript with Oxc and plan exact-span type and symbol queries.
7. Normalize only the TypeScript type structures refinejs can represent
   faithfully. Preserve other rendered types as opaque named types instead of
   guessing their meaning.
8. Combine trusted compiler facts with the selected refinement/effect overlay.
9. Verify refinement obligations. Unknown calls and getters conservatively
   invalidate facts they may have changed.

`refinejs build` uses the same checking pipeline before emitting instrumented
JavaScript, so a build cannot bypass the compiler-backed trust checks.

## Work completed

### Target-aware standard-library model

The former flat prelude was split into a typed catalog with a common
ECMAScript layer and platform layers for:

- `ecmascript`
- `browser`
- `node`
- `deno`, including its Node compatibility surface
- `bun`, including its Node compatibility surface

`auto` target selection parses the source and uses imports plus unbound runtime
globals. It does not grep comments or strings, and local bindings do not count
as platform evidence. Explicit `--target` always wins. Ambiguous sources must
choose a target rather than receiving a nondeterministic mixture of catalogs.

The catalog models more than function signatures. Each entry can describe:

- callback timing: immediate or deferred;
- callback use and materialization behavior;
- receiver effects: none, read, or mutate;
- semantic refinements such as type guards, result length, subset or callback
  element provenance, and receiver length growth;
- receiver containment introduced by mutating calls such as `Array.prototype.push`.

Catalogs and diagnostics use deterministic ordered collections so repeated
runs have stable behavior and output. The verifier looks up catalog entries
directly; it does not inject prelude signatures into user annotations.

### Corsa integration

The CLI turns Corsa on by default. It looks up `CORSA_BIN` / `TSGO`, then
`corsa` or `tsgo` on `PATH`, and the nearest `tsconfig.json` (or a temporary
project that contains the checked file). `--corsa` and `--tsconfig` override
discovery in either `--flag value` or `--flag=value` form; `--no-corsa` skips
TypeScript. Duplicate options are an error.

The Corsa adapter is behind `CompilerTypeProvider`, keeping compiler transport
and project queries outside the verifier. The implementation:

- resolves the project from the exact `tsconfig`;
- runs Corsa from the config directory;
- validates canonical source membership and on-disk content;
- reads normalized compiler options and complete program diagnostics;
- obtains the program source set before scanning implementation files for
  `@ts-ignore`, `@ts-expect-error`, and `@ts-nocheck`;
- records symbol declaration paths so declaration-backed evidence can be
  distinguished from project implementations;
- sorts and deduplicates diagnostics;
- converts compiler UTF-16 positions to refinejs source locations correctly.

The Rust binding is pinned to Corsa `1.12.4`; the executable is intentionally
not bundled. The caller controls which TypeScript project and declaration set
are authoritative.

### Compiler hint planning and type normalization

Oxc source spans are mapped to Corsa source-position requests. Expression
result queries and callable-return queries are separate because they answer
different questions. Queries cover identifiers, statically named members,
object literals, calls, and chains; unsafe computed-member definition guesses
are not made.

Primitive, array, generic, union, object, and function shapes that can be
represented safely are normalized into refinejs types. Complex or unsupported
rendered TypeScript structures remain opaque named types. This is deliberately
less permissive than parsing a type-looking string into semantics the compiler
did not actually establish.

### Trust boundary

Compiler-backed evidence is accepted only when all of the following hold:

- the configured project has no compiler errors;
- JavaScript semantic checking and strict null behavior are enabled;
- no implementation source in the Corsa-known program suppresses diagnostics;
- the symbol used for fallback evidence resolves entirely to declaration files;
- the value is not a local callable or local member implementation without a
  refinejs contract;
- the value has not crossed an unknown execution boundary that invalidates its
  mutable facts;
- a compiler-rendered nominal name has not merely collided with a curated
  catalog type.

These checks prevent several forms of type laundering: hiding project errors
behind suppression comments, treating a local implementation as if it were a
library declaration, or attaching Array/DOM semantics to an unrelated project
type whose display name happens to match.

### Effects and heap invalidation

Curated entries declare their known effects. Everything else that can execute
unknown code—including compiler-only calls and getters—is conservative.
Crossing such a boundary forgets affected heap facts, mutable refinements, and
catalog identities that may no longer be valid.

Immediate and deferred callbacks are modeled separately. An immediate callback
can participate in the current operation's dataflow; a deferred or otherwise
opaque callback is treated as an escape boundary. Nested calls inside callback
bodies are analyzed in evaluation order, including sequence expressions, so
side effects cannot disappear merely because they occur inside a callback
return expression.

The production verifier sends each preprocessed implication directly to Z3
SMT; only `unsat` counts as proof. The former Fixedpoint-plus-SMT path was
removed after corpus differentials found no diagnostic change and consistent
slowdown. An `unknown` result rejects the obligation and is never treated as
proof.

## Provenance model and the three final fixes

Reference provenance is intentionally separate from ordinary logical heap
facts. Heap facts may be invalidated after unknown code executes, but the
history that an unsafe reference may have reached a value must remain monotone.

The verifier records two different relationships:

- **Alias** is bidirectional and means exact reference identity.
- **ContainedBy** is directional and means a value may be contained by a
  receiver or container.

Collapsing these relationships would either lose real taint after mutation or
invent a reverse flow from a container into every inserted value. The final
implementation fixed three related classes of failures:

1. **Containment surviving heap havoc.** Array literals, assignments, mutating
   calls, callback materialization, and cloned/joined branch state add monotone
   provenance edges. Unknown effects can erase refinements but cannot erase
   these edges. Later mutations therefore still see references that previously
   escaped into a container.
2. **One-way containment.** Container flow is `value -> container`; it is not
   automatically reversed. Exact aliases remain bidirectional. This prevents
   false positives where putting a safe value into a tainted container would
   incorrectly make the value an alias of the container.
3. **Callback return provenance.** Callback implementation provenance and the
   provenance of the returned value are tracked separately. Scalar callback
   results do not inherit reference provenance simply because the callback
   closes over or receives a reference, while reference-capable returns (for
   example an `Array<number>`) retain the source needed for later containment
   and mutation checks. Nested callees are not themselves treated as values
   escaping through an outer materializing call.

Local provenance also supports temporary references that have no lexical
binding. This matters for literals and intermediate callback results, which
must not become invisible merely because no variable name refers to them.

## Module map

| Area | Main responsibility |
| --- | --- |
| `src/main.rs` | CLI target/compiler options, path validation, check/build wiring |
| `src/checker.rs` | Compiler-backed orchestration and diagnostic conversion |
| `src/type_provider.rs` | Corsa project boundary, options, diagnostics, source and declaration provenance |
| `src/compiler_hints.rs` | Oxc-to-Corsa query planning and safe type normalization |
| `src/prelude/environment.rs` | Explicit targets and syntax-aware auto-detection |
| `src/prelude/model.rs` | Typed catalog, effects, callback timing, and semantic refinements |
| `src/prelude/catalog.rs` | Common and platform-specific library entries |
| `src/verifier.rs` | Refinement proof, catalog trust, effects, provenance graph, and havoc |
| `src/parser.rs`, `src/syntax.rs` | Annotation and type representation extensions |
| `src/transpiler.rs` | Runtime assertion emission for the expanded checked subset |
| `tests/compiler_backed.rs` | Compiler integration, trust-boundary, effect, and provenance regressions |
| `tests/prelude.rs` | Target catalogs, auto-detection, contracts, and installed-runtime checks |

## Key choices and consequences

| Choice | Reason | Consequence |
| --- | --- | --- |
| Curated semantics plus compiler types | TypeScript declarations do not encode refinement or effect semantics | Broad type coverage does not weaken refinement proof ownership |
| Target overlay independent of `tsconfig` | Runtime semantics and available declarations are different concerns | Users must configure both deliberately |
| Syntax-aware auto-detection | Text matching is fooled by comments, strings, and shadowing | Ambiguity is reported; explicit targets remain predictable |
| External, caller-selected Corsa | The compiler and project configuration are part of the trust input | No hidden compiler download; setup is explicit |
| Whole-project diagnostics and suppression scan | A clean requested file is insufficient if imported implementations are unchecked | More conservative failures, but no suppressed-error fallback evidence |
| Declaration provenance for fallback | Printed types cannot distinguish library declarations from local code | Local implementations require refinejs contracts |
| Catalog identity separate from nominal type text | Project types can collide with standard-library names | Curated semantics cannot be acquired by name alone |
| Conservative unknown effects | Unmodeled calls/getters may mutate reachable state | Some valid programs need contracts or catalog entries |
| Monotone provenance outside heap facts | Havoc should remove stale facts, not erase escape history | Later reference mutations remain soundly connected |
| Separate alias and containment edges | Identity is symmetric; containment is not | Real flows are retained without reverse-flow false positives |
| Opaque preservation of unsupported TS types | Guessing structure is unsound | Some compiler-known types remain unavailable for deeper refinement reasoning |
| Reject unresolved solver results | `unknown` is not a proof | Difficult obligations fail safely |

## Usage

Use an explicit platform target when possible:

```bash
cargo run -- check --target node path/to/source.js
```

Enable compiler-backed checking with an absolute Corsa executable, config, and
source path:

```bash
cargo run -- check \
  --target browser \
  --corsa /absolute/path/to/corsa \
  --tsconfig /absolute/path/to/tsconfig.json \
  /absolute/path/to/source.js
```

For JavaScript projects, the effective compiler configuration must enable
`allowJs`, `checkJs`, and strict checking. Its `lib`, `types`, `typeRoots`, and
included project files determine which declarations Corsa can see. `--target`
does not modify that declaration environment.

## Verification evidence

The implementation commit was checked with:

```text
cargo test --all-targets                  116 passed, 0 failed
cargo fmt --all -- --check                passed
cargo clippy --all-targets -- -D warnings passed
git diff --check                          passed
```

The 116 tests comprise unit tests plus integration suites for compiler-backed
behavior, platform preludes, Flux behavior, and smoke coverage. The
compiler-backed suite exercises type-query planning, UTF-16 offsets, project
configuration, complete diagnostics, suppression rejection, declaration and
local-implementation provenance, catalog-name collisions, unknown-effect
havoc, aliases, directional containment, callback return provenance, branch
joins, nested calls, and real Corsa integration when configured. Platform
runtime execution tests run when the corresponding runtime is installed.

## Known limits

- The curated catalog intentionally covers selected APIs with useful semantic
  refinements; it is not a complete copy of every platform library.
- Corsa is not bundled. Compiler-backed mode requires a compatible executable
  and an exact project configuration.
- Auto-detection cannot infer intent from a source with no unique runtime marker
  or a deliberately mixed environment; use an explicit target in that case.
- Complex TypeScript types that cannot be represented faithfully stay opaque.
- Unknown effects deliberately discard useful mutable facts, so sound code may
  need an explicit refinejs contract or a new curated catalog entry.
- The existing static subset still rejects unsupported JavaScript constructs
  instead of approximating them. In particular, block-bodied contextual
  callbacks are not yet generally modeled.
- Runtime integration coverage depends on the relevant browser/server runtime
  being available in the test environment.

These are explicit boundaries. None of them changes the rule that a refinement
is accepted only when refinejs can prove it under the facts that remain valid.
