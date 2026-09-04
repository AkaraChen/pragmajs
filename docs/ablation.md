# Ablation notebook

Date: 2026-09-04. Baseline snapshot: `055b30d`.

This notebook treats every abstraction as a hypothesis. A green regression
suite is not the outcome metric: each run is evaluated against per-file gold
labels, and a diagnostic changing to an unrelated error is not counted as the
same capability.

## Protocol

Labels have four meanings:

- `ACCEPT`: valid input; any diagnostic is `lost_valid`.
- `REJECT:<rule>`: invalid input; losing the named rule is either
  `escaped_invalid` (no diagnostic) or `reason_changed` (only unrelated
  diagnostics remain).
- `OUT_OF_DOMAIN:<guard>`: an intentionally unsupported construct. Removing
  the guard is not a precision improvement.
- `INVALID` / `COMPILER_REQUIRED`: reserved for compiler-filtered and sparse
  annotation corpora. They are not mixed into checker-only precision/recall.

The ownership runner changes transfer behavior before checking. It does not
filter a completed diagnostic list. The refinement runner selects a solver
before obligations are discharged. One-factor-at-a-time (OAT) results are
screening evidence, not independent causal effects where features depend on
one another.

Reproduce:

```sh
cargo run -p pragma-own --example ablation
cargo run -p pragma-own --example ablation -- --csv
cargo run -p pragma-rt --example ablation
ABLATION_ROUNDS=5 cargo run -p pragma-rt --example ablation
```

## Ownership: first OAT screen

Corpus: 42 cases: 19 `ACCEPT`, 21 `REJECT`, and 2 `OUT_OF_DOMAIN`.
The manifest is [`crates/own/ablation/manifest.tsv`](../crates/own/ablation/manifest.tsv).

| Variant | Valid kept | Lost valid | Invalid caught | Escaped invalid | Reason changed | OOD guarded | Changed cases |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 15 | 4 | 16 | 5 | 0 | 2 | 0 |
| no function contracts | 16 | 3 | 3 | 16 | 2 | 1 | 19 |
| no move tracking | 18 | 1 | 0 | 19 | 2 | 1 | 22 |
| no exact-once | 17 | 2 | 11 | 10 | 0 | 2 | 11 |
| no affine kind | 15 | 4 | 15 | 6 | 0 | 2 | 1 |
| no borrow model | 14 | 5 | 11 | 7 | 3 | 2 | 6 |
| no local borrow directives | 14 | 5 | 14 | 5 | 2 | 2 | 3 |
| no local clone directives | 14 | 5 | 16 | 5 | 0 | 2 | 1 |
| no local drop directives | 14 | 5 | 16 | 5 | 0 | 2 | 1 |
| no local kind directives | 15 | 4 | 14 | 7 | 0 | 2 | 2 |
| no local callee contracts | 15 | 4 | 15 | 6 | 0 | 2 | 1 |
| no owned-return propagation | 16 | 3 | 14 | 7 | 0 | 2 | 3 |
| no instance dispatch | 14 | 5 | 16 | 5 | 0 | 2 | 1 |
| no control-flow splitting | 14 | 5 | 15 | 6 | 0 | 2 | 2 |
| no loop depth | 15 | 4 | 15 | 6 | 0 | 2 | 1 |
| no non-consuming paths | 15 | 4 | 17 | 4 | 0 | 2 | 5 |
| no unknown-call conservatism | 15 | 4 | 15 | 6 | 0 | 2 | 1 |
| no optional-call paths | 16 | 3 | 17 | 4 | 0 | 2 | 2 |
| no unmapped guards | 16 | 3 | 16 | 5 | 0 | 0 | 3 |
| no runtime prelude | 14 | 5 | 15 | 6 | 0 | 2 | 4 |

Every implemented axis changes at least one case. The small improvements in
`valid kept` are not wins: for example, disabling move tracking removes false
positives only by letting all move errors escape. Likewise, accepting `eval`
after removing unmapped guards is a domain escape.

The important positive evidence is bidirectional:

- exact-once uniquely catches unused owned values;
- affine differs from both Copy and Unique;
- borrow state is necessary for overlap and lifetime errors, and removing it
  also breaks valid lexical-borrow programs;
- branch splitting is needed both to reject one-branch consumption and to
  accept both-branch consumption; the ablated transfer is an explicit
  source-order walk of all branches;
- loop definition depth, local call effects, owned returns, instance receiver
  effects, unknown-call conservatism, and the runtime prelude each have a
  direct witness;
- the former `local-directives` switch was exactly equivalent to disabling its
  borrow, clone, drop, and kind components together, so the redundant master
  state was removed. Each component now has its own witness;
- treating every optional call as a non-consuming path is too coarse: when the
  callee is a known local function, `consume?.(value)` must consume on the only
  feasible path. Removing the approximation catches one existing false
  negative. A second known-callee witness is valid when its only use is
  `consume?.(value)`, but the same syntax-only split invents an infeasible
  skipped branch and rejects it. Both results require callee nullability rather
  than punctuation alone. A truly nullable-callee witness remains out of reach
  until the ownership contract language can express a callable parameter;
- `Apps.path` is a composite abstraction (member heads, Copy/ref arguments,
  known variadics, and optional calls), so its row cannot identify which path
  source is valuable.

Dependencies that prevent a naive causal reading include:

```text
function contracts -> local callee contracts
local borrow directives -> lexical borrow and shorthand borrow
callee contracts -> owned-return propagation
instance dispatch -> receiver effects and some owned returns
unknown-call conservatism x non-consuming paths
```

The runner also evaluates five complete 2x2 cells. The contrast below is
`y11 - y10 - y01 + y00`; nonzero values show that OAT deltas are not additive.

| Interaction | Valid kept | Lost valid | Invalid caught | Escaped invalid | Reason changed |
|---|---:|---:|---:|---:|---:|
| function contracts x local callee contracts | 0 | 0 | +1 | -1 | 0 |
| borrow model x local borrow directives | +1 | -1 | +2 | 0 | -2 |
| owned return x instance dispatch | 0 | 0 | 0 | 0 | 0 |
| unknown-call conservatism x non-consuming paths | 0 | 0 | +1 | -1 | 0 |
| move tracking x exact-once | -2 | +2 | +5 | -5 | 0 |

The first, second, fourth, and fifth pairs empirically confirm coupling; the
owned-return/instance pair is additive on this corpus, not proven independent.
The next run must split the remaining composite `borrow-model` and `Apps.path`
axes.

## Ownership: baseline contradictions

The gold corpus contradicts the current implementation even with every feature
enabled. These are not missing diagnostics in the runner; each has a minimized
fixture under `crates/own/ablation/fixtures`.

| Current abstraction | Gold result | Counterexample |
|---|---|---|
| `HashMap<String, VarEntry>` | false negative | an inner shadow overwrites the outer owned binding; popping it does not restore the outer binding |
| positional contracts after `filter_map(ident_of_pattern)` | false negative | a destructured first parameter shifts the ownership type of the second parameter |
| identifier-only transfer | false negative | `destination = resource` consumes the source but creates no owned destination |
| scalar-only ownership | false negative | `{ resource }` consumes the source into an untracked aggregate which can then be forgotten |
| name-only capture scan | false positive | an arrow parameter shadowing an outer owned name is reported as a capture |
| file-global `HashMap<String, FnSig>` | false positive | a nested same-name function overwrites an unrelated top-level callee contract |
| overload collapse + `is_fs_readfile_callback` | false positive | callback `fs.readFile` bound to a variable is modeled as returning a unique `Buffer` |
| syntax-only optional-call path splitting | false negative and false positive | a definitely bound local consumer is treated as skippable, both missing later reuse and inventing a forget when it is the only use |

The observed baseline is therefore 15/19 valid cases retained and 16/21
invalid cases caught. The legacy `pragma-own` suite still passes (123 tests),
which demonstrates why its mostly `contains(...)` assertions cannot serve as
precision/recall gold.

Replacement experiments, in order:

1. scoped binding/symbol IDs instead of name-keyed state and signatures;
2. conservative rejection of assignment/aggregate escape, followed by real
   ownership propagation;
3. overload-aware call resolution, deleting the `fs.readFile` special case;
4. CFG reachability/state sets instead of a syntax walk and one chosen branch
   table;
5. one context-aware AST visitor instead of duplicated collect/count/discard/
   exclusive/capture walkers.

## Refinements: Fixedpoint ablation

The original solver path atomized abstract predicates, built one non-recursive
Horn rule for `assumptions => consequent`, asked Z3 Fixedpoint, and still fell
back to quantifier-free SMT for `Unsat` or `Unknown`. The ablation kept the
same predicate atomization, congruence constraints, and Int-to-Number axioms,
but sent the implication directly to SMT.

The pre-intervention corpus contained 33 Flux positive, 67 Flux negative,
7 prelude positive, and 25 prelude negative fixtures (132 total), without
Corsa. Timing is the median of three full-corpus runs and includes equal
parsing overhead.

| Solver | Valid kept | Lost valid | Invalid caught | Escaped invalid | Median |
|---|---:|---:|---:|---:|---:|
| Fixedpoint + SMT fallback | 40 | 0 | 92 | 0 | 10,244.948 ms |
| direct SMT | 40 | 0 | 92 | 0 | 9,234.629 ms |

Acceptance changes: **0**. Exact diagnostic-list changes: **0**. Direct SMT
was 9.86% faster in this run. Direct SMT then became the production default,
while the legacy backend remained explicitly selectable for another
differential run.

The congruence experiment added one directed positive fixture, bringing the
current corpus to 133 files. A one-round rerun (timing is noisier than the
three-round table above) produced:

| Configuration | Valid kept | Lost valid | Invalid caught | Escaped invalid | Exact diagnostic lists changed |
|---|---:|---:|---:|---:|---:|
| direct SMT (production) | 41 | 0 | 92 | 0 | 0 |
| Fixedpoint + SMT fallback (legacy) | 41 | 0 | 92 | 0 | 0 |
| no Int-to-Number conversion axioms | 34 | 7 | 92 | 0 | 14 |
| no abstract-predicate congruence | 40 | 1 | 92 | 0 | 1 |

Direct SMT took 9,350.866 ms versus 11,262.790 ms for the legacy path in this
run (about 17.0% faster), again with identical diagnostics. The conversion
bridge is not removable: without it, seven valid programs are rejected,
spanning constant branches, dense arrays, polymorphism, vector literals, and
the common Array prelude. Predicate congruence is also necessary: the new
parser-to-checker witness establishes `p(x)`, aliases boolean `y` to `x`, and
returns `y` under the contract `p($)`. Removing congruence rejects exactly this
valid file. A separate test confirms that domain/sort validation remains active
when congruence itself is off.

A final three-round release-mode run again found zero acceptance and exact
diagnostic differences. Median full-corpus time was 8,363.208 ms for direct
SMT and 10,421.700 ms for Fixedpoint plus fallback, making the direct path
about 19.8% faster. The Fixedpoint branch, its selector APIs, quantifier
construction, and variable collector were then deleted. The pre-deletion
differential remains reproducible at commit `8479e6c`; keeping an inert runtime
abstraction on the current branch is not required for reproducibility.

Static use analysis also found abstractions with no verifier consumer:

- `FunctionSignature.type_parameters`;
- `PropertySignature.readonly`;
- `SemanticRefinement::{TypeGuard, ResultElementsFromCallback,
  ResultElementsSubsetOfReceiver}` (explicit no-op match arms);
- `ReceiverEffect::Read` versus no receiver effect;
- `writes_ambient_state` for APIs without a receiver;
- private dead helpers `number_pair`, `number_compare`, and `as_number_term`
  (now deleted after a zero-consumer search and the full RT suite).

A zero delta for these fields currently means “unimplemented or uncovered”,
not “their semantics are preserved for free”. Remove them or first add a real
consumer and a witness.

## Integration ablations

These measurements use the committed `055b30d` CLI binary, so the experimental
feature switches above do not affect them.

| Corpus | Configuration | Result |
|---|---|---|
| 13 ownership `ok-*` examples | `pragma-own` library | 13/13 pass |
| same | unified CLI, Corsa off | 1/13 pass; 12 rejected by refinement local-contract checks |
| same | unified CLI, Corsa auto | 0/13 pass; 25 TypeScript errors plus one refinement error |
| 40 refinement positive fixtures | Corsa off, own runtime none | 40/40 pass |
| same | Corsa off, own runtime node | 39/40; DOM `response` gets an ownership `unique-forget` |
| `sqrt.js` | Corsa off / auto | pass / TS7006 failure |
| `mixed-ok.js` | Corsa off / auto | pass / three TS7006 failures |
| compiler browser fixture | Corsa off / auto | three missing-type errors / pass |

The unified layer is therefore not neutral composition: parse, ownership,
refinement, and TypeScript acceptance are combined with logical AND. Changing
an unrelated checker changes the corpus label. Corsa is valuable when evidence
is requested but costs roughly 0.28 s -> 10.02 s on the 13 cached ownership
examples and rejects otherwise explicit programs under ordinary TypeScript
strictness.

Other integration contradictions:

- `--target deno|bun` does not select the corresponding ownership runtime, so
  platform ownership errors can silently disappear;
- one compiler/provider error aborts a directory before later files run;
- a pragmajs Auto-mode test uses a nonexistent filename, silently degrades to
  Corsa Off, and therefore tests a different path from the real CLI;
- “parse once” is only true with Corsa Off; compiler suppression checks parse
  implementation files again;
- diagnostic columns mix Unicode scalar, UTF-8 byte, and UTF-16 units. With an
  emoji before a call, the same source position can be reported as columns 24,
  25, or 27 depending on the producer.

This motivated an executable matrix with explicit
`checker in {own, rt, all}`, compiler mode, and separate ownership-runtime and
refinement-target axes. Each cell records own/rt/TS/provider diagnostics,
frontend parse count, and wall time. The unified CLI exit code remains a
compatibility observation, not an outcome metric for either checker.

### Checker-selection intervention

The first integration intervention adds `--checker all|own|rt` while retaining
`all` as the compatibility default. It prevents the unselected annotation
parser, checker, and compiler-offset requests from running. In `own` mode,
Auto compiler discovery is skipped when there are no sparse ownership payloads;
an explicit compiler configuration still runs. An own-only build no longer
injects the refinement runtime.

Real-path rerun after the intervention:

| Corpus/configuration | own | rt | all |
|---|---:|---:|---:|
| 13 ownership `ok-*`, Corsa off | 13/13 | 1/13 | 1/13 |
| 33 Flux positive, Corsa off, own runtime none | 33/33 | 33/33 | 33/33 |
| 7 prelude positive, runtime none | — | 7/7 | 7/7 |
| 7 prelude positive, runtime node | — | 7/7 | 6/7 |

The own-only Auto rerun is now 13/13 rather than 0/13 because no unused
TypeScript gate is started. These results prove that checker selection removes
cross-checker pollution; they also show why `all` cannot be used to estimate
either checker's precision. Platform coupling and the default `all` policy
remain separate hypotheses.

### Executable integration matrix

`cargo run -p pragmajs --example ablation` now evaluates 19 labeled cells with
a deterministic injected compiler provider, so it does not depend on an
installed Corsa binary. All cells reuse one frontend parse and preserve the
five diagnostic producers as separate CSV columns.

| Contrast | Producer counts / result |
|---|---|
| same cross-checker fixture under own / rt / all | `(own, rt) = (1,0) / (0,1) / (1,1)` |
| invalid refinement, compiler off / compiler error | `(rt, compiler) = (1,0) / (0,1)`; compiler error short-circuits verification |
| own-only with explicit compiler error | compiler diagnostic is observable, but compatibility `combined_failed` remains false |
| sparse RT, off / supplied evidence | RT diagnostics `2 -> 0` |
| sparse ownership, off / supplied evidence | ownership diagnostics `2 -> 1`; missing type disappears but the real forget remains |
| Bun source: node runtime x node target | `(own, rt) = (0,1)` |
| Bun source: bun runtime x node target | `(1,1)` |
| Bun source: node runtime x bun target | `(0,0)`, demonstrating the silent ownership hole |
| Bun source: bun runtime x bun target | `(1,0)` |

The platform 2x2 proves that `runtime` and `target` are independent knobs even
though users normally intend one platform. A unified profile is the next
intervention; the mismatched cells remain in the matrix as regression
controls.
