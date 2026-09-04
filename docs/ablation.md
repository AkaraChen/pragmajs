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

Corpus: 37 cases: 17 `ACCEPT`, 18 `REJECT`, and 2 `OUT_OF_DOMAIN`.
The manifest is [`crates/own/ablation/manifest.tsv`](../crates/own/ablation/manifest.tsv).

| Variant | Valid kept | Lost valid | Invalid caught | Escaped invalid | Reason changed | OOD guarded | Changed cases |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 14 | 3 | 14 | 4 | 0 | 2 | 0 |
| no function contracts | 14 | 3 | 1 | 15 | 2 | 1 | 18 |
| no move tracking | 16 | 1 | 0 | 16 | 2 | 1 | 19 |
| no exact-once | 15 | 2 | 11 | 7 | 0 | 2 | 8 |
| no affine kind | 14 | 3 | 13 | 5 | 0 | 2 | 1 |
| no borrow model | 13 | 4 | 9 | 6 | 3 | 2 | 6 |
| no local directives | 12 | 5 | 12 | 4 | 2 | 2 | 4 |
| no local callee contracts | 14 | 3 | 13 | 5 | 0 | 2 | 1 |
| no owned-return propagation | 15 | 2 | 12 | 6 | 0 | 2 | 3 |
| no instance dispatch | 13 | 4 | 14 | 4 | 0 | 2 | 1 |
| no control-flow splitting | 13 | 4 | 13 | 5 | 0 | 2 | 2 |
| no loop depth | 14 | 3 | 13 | 5 | 0 | 2 | 1 |
| no non-consuming paths | 13 | 4 | 14 | 4 | 0 | 2 | 3 |
| no unknown-call conservatism | 14 | 3 | 13 | 5 | 0 | 2 | 1 |
| no unmapped guards | 15 | 2 | 14 | 4 | 0 | 0 | 3 |
| no runtime prelude | 13 | 4 | 13 | 5 | 0 | 2 | 4 |

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
- `Apps.path` is a composite abstraction (member heads, Copy/ref arguments,
  known variadics, and optional calls), so its row cannot identify which path
  source is valuable.

Dependencies that prevent a naive causal reading include:

```text
function contracts -> local callee contracts
local directives -> lexical borrow and shorthand borrow
callee contracts -> owned-return propagation
instance dispatch -> receiver effects and some owned returns
unknown-call conservatism x non-consuming paths
```

The runner also evaluates five complete 2x2 cells. The contrast below is
`y11 - y10 - y01 + y00`; nonzero values show that OAT deltas are not additive.

| Interaction | Valid kept | Lost valid | Invalid caught | Escaped invalid | Reason changed |
|---|---:|---:|---:|---:|---:|
| function contracts x local callee contracts | 0 | 0 | +1 | -1 | 0 |
| borrow model x local directives | +1 | -1 | +2 | 0 | -2 |
| owned return x instance dispatch | 0 | 0 | 0 | 0 | 0 |
| unknown-call conservatism x non-consuming paths | 0 | 0 | +1 | -1 | 0 |
| move tracking x exact-once | -1 | +1 | +3 | -3 | 0 |

The first, second, fourth, and fifth pairs empirically confirm coupling; the
owned-return/instance pair is additive on this corpus, not proven independent.
The next run must split the composite `local-directives`, `borrow-model`, and
`Apps.path` axes.

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

The observed baseline is therefore 14/17 valid cases retained and 14/18
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

The current solver path atomizes abstract predicates, builds one non-recursive
Horn rule for `assumptions => consequent`, asks Z3 Fixedpoint, and still falls
back to quantifier-free SMT for `Unsat` or `Unknown`. The ablation keeps the
same predicate atomization, congruence constraints, and Int-to-Number axioms,
but sends the implication directly to SMT.

Corpus: all 33 Flux positive + 67 Flux negative + 7 prelude positive + 25
prelude negative fixtures (132 total), without Corsa. Timing is the median of
three full-corpus runs and includes equal parsing overhead.

| Solver | Valid kept | Lost valid | Invalid caught | Escaped invalid | Median |
|---|---:|---:|---:|---:|---:|
| Fixedpoint + SMT fallback | 40 | 0 | 92 | 0 | 10,244.948 ms |
| direct SMT | 40 | 0 | 92 | 0 | 9,234.629 ms |

Acceptance changes: **0**. Exact diagnostic-list changes: **0**. Direct SMT
was 9.86% faster in this run. This is strong evidence to delete the Fixedpoint
layer, subject to repeating the timing on CI and adding solver-call counters;
it is not a claim about workloads outside this corpus.

Two additional solver assumptions use the same 132-file corpus. A one-round
rerun (timing is noisier than the three-round table above) produced:

| Configuration | Valid kept | Lost valid | Invalid caught | Escaped invalid | Exact diagnostic lists changed |
|---|---:|---:|---:|---:|---:|
| baseline | 40 | 0 | 92 | 0 | 0 |
| no Int-to-Number conversion axioms | 33 | 7 | 92 | 0 | 14 |
| no abstract-predicate congruence | 40 | 0 | 92 | 0 | 0 |

The conversion bridge is not removable: without it, seven valid programs are
rejected, spanning constant branches, dense arrays, polymorphism, vector
literals, and the common Array prelude. Predicate congruence is different: a
directed solver test proves it matters for `x same y && p(x) => p(y)`, and a
second test proves domain/sort validation remains active when congruence is
off, but no end-to-end corpus file exercises that law. Its zero delta is a
coverage failure, not deletion evidence; a labeled end-to-end witness is now
required before deciding its fate.

Static use analysis also found abstractions with no verifier consumer:

- `FunctionSignature.type_parameters`;
- `PropertySignature.readonly`;
- `SemanticRefinement::{TypeGuard, ResultElementsFromCallback,
  ResultElementsSubsetOfReceiver}` (explicit no-op match arms);
- `ReceiverEffect::Read` versus no receiver effect;
- `writes_ambient_state` for APIs without a receiver;
- private dead helpers `number_pair`, `number_compare`, and `as_number_term`.

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

The next integration experiment needs explicit `checker in {own, rt, all}`,
`compiler in {off, explicit}`, and one unified platform profile. Each cell must
record own/rt/TS/provider diagnostics separately, parse count, and wall time.
Until then, the unified CLI exit code is not a valid outcome metric for either
checker.

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
