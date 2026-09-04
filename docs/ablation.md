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

## Ownership: current OAT screen

Corpus: 43 cases: 19 `ACCEPT`, 22 `REJECT`, and 2 `OUT_OF_DOMAIN`.
The manifest is [`crates/own/ablation/manifest.tsv`](../crates/own/ablation/manifest.tsv).

| Variant | Valid kept | Lost valid | Invalid caught | Escaped invalid | Reason changed | OOD guarded | Changed cases |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 19 | 0 | 22 | 0 | 0 | 2 | 0 |
| no function contracts | 17 | 2 | 3 | 17 | 2 | 1 | 22 |
| no move tracking | 18 | 1 | 0 | 20 | 2 | 1 | 24 |
| no exact-once | 19 | 0 | 12 | 10 | 0 | 2 | 14 |
| no affine kind | 19 | 0 | 20 | 2 | 0 | 2 | 2 |
| no borrow model | 18 | 1 | 17 | 2 | 3 | 2 | 6 |
| no local borrow directives | 18 | 1 | 20 | 0 | 2 | 2 | 3 |
| no local clone directives | 18 | 1 | 22 | 0 | 0 | 2 | 1 |
| no local drop directives | 18 | 1 | 22 | 0 | 0 | 2 | 1 |
| no local kind directives | 19 | 0 | 19 | 3 | 0 | 2 | 3 |
| no local callee contracts | 17 | 2 | 20 | 2 | 0 | 2 | 4 |
| no owned-return propagation | 19 | 0 | 20 | 2 | 0 | 2 | 2 |
| no instance dispatch | 18 | 1 | 22 | 0 | 0 | 2 | 1 |
| no control-flow splitting | 18 | 1 | 21 | 1 | 0 | 2 | 2 |
| no loop depth | 19 | 0 | 21 | 1 | 0 | 2 | 1 |
| no non-consuming paths | 16 | 3 | 20 | 2 | 0 | 2 | 5 |
| no unknown-call conservatism | 19 | 0 | 21 | 1 | 0 | 2 | 1 |
| no optional-call paths | 19 | 0 | 21 | 1 | 0 | 2 | 1 |
| no unmapped guards | 19 | 0 | 22 | 0 | 0 | 0 | 2 |
| no runtime prelude | 17 | 2 | 21 | 1 | 0 | 2 | 3 |

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
- optional-call paths are necessary for an unknown optional method:
  `holder.consume?.(value)` may skip the consume, so removing the split loses
  its `unique-forget`. Conversely, `consume?.(value)` is definite when
  `consume` resolves to a declared local function. Making the transfer
  callee-sensitive removes both of the previous syntax-only contradictions
  while retaining a one-case ablation witness;
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
| function contracts x local callee contracts | +2 | -2 | +2 | -2 | 0 |
| borrow model x local borrow directives | +1 | -1 | +2 | 0 | -2 |
| owned return x instance dispatch | 0 | 0 | 0 | 0 | 0 |
| unknown-call conservatism x non-consuming paths | 0 | 0 | +1 | -1 | 0 |
| move tracking x exact-once | 0 | 0 | +10 | -10 | 0 |

The first, second, fourth, and fifth pairs empirically confirm coupling; the
owned-return/instance pair is additive on this corpus, not proven independent.
The next run must split the remaining composite `borrow-model` and `Apps.path`
axes.

## Ownership: resolved baseline contradictions

The first gold run exposed eight faulty abstractions across nine baseline
counterexamples even with every feature enabled. Each has a minimized fixture
under `crates/own/ablation/fixtures`; all are now resolved on this corpus.

| Former abstraction | Gold result | Counterexample |
|---|---|---|
| overwrite-only `HashMap<String, VarEntry>` scopes | false negative | an inner shadow erases the outer owned binding instead of restoring it on scope exit |
| positional contracts after filtering destructured parameters | false negative | a destructured first parameter shifts the ownership type of the second parameter |
| identifier-only declarations | false negative | `destination = resource` consumes the source but creates no owned destination |
| scalar-only ownership | false negative | `{ resource }` consumes the source into an untracked aggregate which can then be forgotten |
| name-only capture scan | false positive | an arrow parameter shadowing an outer owned name is reported as a capture |
| file-global `HashMap<String, FnSig>` | false positive | a nested same-name function overwrites an unrelated top-level callee contract |
| overload collapse + `is_fs_readfile_callback` | false positive | callback `fs.readFile` bound to a variable is modeled as returning a unique `Buffer` |
| syntax-only optional-call splitting | false negative and false positive | a definitely bound local consumer gets an infeasible skipped path, while later reuse can miss the definite consume |

The observed baseline now retains all 19/19 valid cases, catches all 22/22
invalid cases, and guards both 2/2 out-of-domain cases. The `pragma-own`
package suite also passes (149 tests). Earlier package suites stayed green
while these gold contradictions remained, which demonstrates why ordinary
regression assertions cannot serve as a precision/recall corpus by themselves.

The interventions removed contradictions without hiding them behind a
different diagnostic. Scope frames now retain and restore the `VarEntry` they
shadow: the baseline moved from `18/22` to `19/22` invalid cases caught, with
no other gold bucket changing. This fixes scope exit, but expression lookup
and borrow owners are still name-keyed. The
callee-sensitive optional-call transfer then turned both known-callee
counterexamples into their gold outcomes; the new unknown-method fixture keeps
the optional-path feature measurable. Finally, preserving the original AST
index while skipping a destructured parameter moved the baseline from `19/22`
to `20/22` invalid cases caught and fixed both ordinary and arrow functions.
A deliberately narrow assignment transfer for discarded simple `=` statements
then moved it to `21/22`: the destination inherits the live obligation, a
tracked value overwritten there is settled first, self-assignment stays live,
and unsafe nested-untracked/value-producing contexts remain outside the
intervention rather than receiving guessed scope semantics.

The final escaped-invalid fixture moved a unique value into an object and then
forgot the object. Direct identifier values in object/array literals (including
nested literals) now create one opaque aggregate owner, which can move or be
consumed as a whole. Computed keys, consuming calls, spread, newly produced
owned call results, and field extraction are deliberately not promoted into
this abstraction; regression tests prevent consumed call arguments and keys
from creating false container obligations. This took invalid recall to 22/22
on the current corpus, but it is not a substitute for path-sensitive heap
ownership.

Capture candidates now subtract function parameters, function-local `var`
bindings, lexical block declarations, and catch parameters before the existing
capture walk. Reverse tests ensure that a block-local shadow does not hide a
real capture after the block. This resolves the current arrow counterexample,
but `for` headers, switch cases, and class methods still motivate replacing the
manual walkers with symbol IDs.

Bare local callable contracts are now pre-collected per program/function owner
and resolved from the innermost owner outward. This preserves declaration
hoisting while preventing nested and sibling functions from overwriting one
another. It deliberately does not claim full block or dotted-member symbol
resolution. Finally, the generated Node catalog now corrects callback-only
`fs.readFile` to `void`; the checker-side spelling/argument-shape exception was
deleted, while promise and synchronous reads retain owned returns. The
generator still collapses overloads generally, so this named correction is a
reproducible boundary rather than an overload resolver.

Replacement experiments, in order:

1. symbol IDs for expression lookup, captures, borrow owners, and signatures;
2. path-sensitive heap ownership, then extending assignment propagation beyond
   discarded simple identifier statements;
3. overload-aware call resolution, replacing the generator's known
   `fs.readFile` correction;
4. CFG reachability/state sets instead of a syntax walk and one chosen branch
   table;
5. one context-aware AST visitor instead of duplicated collect/count/discard/
   exclusive/capture walkers.

The `checked_bodies` set was also challenged directly. Disabling it left the
43-case gold multiset unchanged, but four package tests gained real
`unique-forget` false positives. The same arrow body is currently reachable
first through a declaration/property-aware path and then through a generic
contained-expression path; the second path lacks the annotation offsets and
misclassifies an owned return as a discard. The set is therefore not merely a
diagnostic deduplicator. Removing it first requires one structural body pass
that preserves parent annotation context; deduplicating emitted messages would
hide duplicated state and capture effects.

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

### Heap-fact invalidation intervention

The 133-file corpus could not distinguish `FunctionEffects.writes_ambient_state`
from no effect, but static reachability showed why that result was misleading.
The field's only verifier read lived inside a receiver snapshot: all static API
annotations were unreachable, deferred callbacks already requested a snapshot,
and only `Bun.Server.stop` independently reached the branch. At the same time,
RT annotations could not spell qualified catalog types such as `Bun.Server`, so
the corpus had no source-level way to exercise that receiver.

Qualified named base types are now parsed directly, including qualified generic
names. An explicit qualified contract opts into catalog identity only when the
receiver or property actually exists in the selected registry. This enabled a
new negative fixture: from `xs: DenseArray<number>[1]`, call `server.stop()` and
then `xs.pop()`. Keeping the stale length fact would accept the program.

A three-round release run on the resulting 134 files produced:

| Configuration | Valid kept | Lost valid | Invalid caught | Escaped invalid | Exact diagnostic lists changed | Median |
|---|---:|---:|---:|---:|---:|---:|
| direct SMT (production) | 41 | 0 | 93 | 0 | 0 | 8,628.673 ms |
| no Int-to-Number conversion axioms | 34 | 7 | 93 | 0 | 14 | 8,052.724 ms |
| no abstract-predicate congruence | 40 | 1 | 93 | 0 | 1 | 8,751.028 ms |
| no heap-fact invalidation | 41 | 0 | 92 | 1 | 1 | 9,774.558 ms |

The heap ablation changes exactly the new fixture. The field was renamed to
`invalidates_heap_facts`, and invalidation is now explicit rather than a side
effect of snapshotting a fictitious receiver length. Only `Bun.Server.stop`
sets it. Eight unreachable ambient writes, one redundant `Bun.serve` write,
four callback assignments already covered by callback timing, six explicit
default reads, and both ambient helper constructors were removed. A separate
diagnostic dump confirmed an empty exact diff on all original 133 files; the
old snapshot placement and the explicit operation were also exactly equivalent
on all 134.

### Static catalog cleanup

Static use analysis also found abstractions with no verifier consumer. The
initial four groups deleted were:

- private helpers `number_pair`, `number_compare`, and `as_number_term`;
- `FunctionSignature.type_parameters`, its builder, and 18 catalog writes;
- `SemanticRefinement::{TypeGuard, ResultElementsFromCallback,
  ResultElementsSubsetOfReceiver}`, six catalog writes, three explicit no-op
  verifier arms, and a storage-only assertion;
- `PropertySignature.readonly` and its 16 catalog writes.

The last two were removed as one catalog-metadata intervention: 60 net lines
disappeared, all 103 RT tests passed, and normalized full diagnostic output for
the 133-file corpus had an empty diff. This is evidence that the fields had no
current semantics, not that type guards, element relations, or readonly writes
are implemented for free. They need a new design and a witness before they
return.

Subsequent interventions removed more storage and API surface, each with an
empty exact diagnostic diff:

- the single-field `PropertySignature` wrapper (38 net lines);
- five full-registry enumeration APIs. None had a production consumer, and the
  only `globals()`/`modules()` test merely repeated `BTreeMap`'s ordering
  guarantee (41 net lines including that test);
- `ReceiverEffect::Read`: all 13 construction sites were indistinguishable from
  `None`, because the verifier branches only on `Mutate`;
- eleven explicit `.with_effects(default)` catalog chains left after removing
  `Read`;
- a computed-callee match arm identical to the wildcard fallback;
- the zero-call `compiler_hints::analyze_program` convenience wrapper; callers
  that already parsed a program can use `analyze_program_with_offsets(...,
  &[])`;
- `registry_for_source`, whose only consumer was a test now exercising the
  production parse-once `registry_for_program` path instead.

These are source/API simplifications, not claims that readonly effects or
catalog enumeration have been implemented elsewhere. In particular, deleting
the public registry iterators and convenience wrappers is a pre-1.0 API break.
The qualified-type and heap witness above show the opposite outcome: when
static analysis found one reachable consumer, the abstraction was narrowed and
tested rather than deleted wholesale.

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

Other integration contradictions identified by the first screen:

- independent `--runtime` and `--target` controls can describe different
  platforms, so ownership errors can silently disappear;
- one compiler/provider error aborts a directory before later files run;
- the original pragmajs Auto-mode coverage used a nonexistent filename,
  silently degraded to Corsa Off, and therefore tested a different path from
  the real CLI; the checker-selection intervention added a real-file test;
- “parse once” is only true with Corsa Off; compiler suppression checks parse
  implementation files again;
- diagnostic columns mixed Unicode scalar, UTF-8 byte, and UTF-16 units. An
  astral character is required to distinguish all three; this was intervened
  on below.

This motivated an executable matrix with explicit
`checker in {own, rt, all}`, compiler mode, coherent platform profiles, and
separate low-level ownership-runtime/refinement-target controls. Each cell
records own/rt/TS/provider diagnostics, frontend parse count, and wall time.
The unified CLI exit code remains a compatibility observation, not an outcome
metric for either checker.

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

`cargo run -p pragmajs --example ablation` now evaluates 28 labeled cells with
a deterministic injected compiler provider, so it does not depend on an
installed Corsa binary. All cells reuse one frontend parse and preserve the
five diagnostic producers as separate CSV columns. Where a producer exposes a
structured location, its `line:column` is retained rather than erased into the
message.

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
| same source under coherent Bun profile | `(1,0)`; runtime and target resolve to Bun atomically |
| Unicode ownership / RT / compiler / parse cells | exact scalar locations `1:75 / 2:87 / 3:10 / 1:26` |

The platform 2x2 proves that `runtime` and `target` are independent knobs even
though users normally intend one platform. The intervention adds
`--platform ecmascript|browser|node|deno|bun`, which maps both checker preludes
as one choice and rejects combinations with either low-level override. There
is deliberately no incomplete `auto` profile, and the historical default is
unchanged for compatibility. All five profile cells hit gold; the four
mismatched cells remain executable regression controls rather than normal
user-facing configurations.

### Location-unit intervention

The ambiguous `offset_to_line_col` API was replaced by explicit UTF-8-byte and
UTF-16-code-unit inputs; both now produce 1-based Unicode-scalar columns. Own,
RT annotation parsing, and RT AST diagnostics use the byte entry point, while
compiler diagnostics use the UTF-16 entry point. This leaves existing own
locations unchanged. For every preceding `🙂`, RT verifier locations move
three columns left relative to the old byte count, and compiler locations move
one column left relative to the old UTF-16 count.

Four matrix cells put `🙂` before one finding from each producer with a source
span. Their exact positions participate in `matches_gold`, so a future
byte/code-unit regression fails the runner even though acceptance counts do
not change. The deterministic compiler provider also stopped casting byte
offsets directly to UTF-16.

The final frontend intervention preserves Oxc parse diagnostics as structured
values through `pragma-parse` and `CombinedCheck`. Location extraction chooses
a primary label when Oxc supplies one and otherwise falls back to the first
label; diagnostics without any label keep no invented position. The Unicode
parse fixture exposes Oxc byte offset 28 as scalar location `1:26`, bringing
parse errors under the same executable gold check as own, RT, and compiler
diagnostics.
