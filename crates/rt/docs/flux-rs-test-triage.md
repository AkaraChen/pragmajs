# flux-rs test triage

Review of [flux-rs](https://github.com/flux-rs/flux) `tests/tests/{pos,neg}/surface`
and `tests/tests/pos/vec` (main, fetched for this port). Each surface `.rs` file is
either **ported** (first wave plus the remainder that had an honest JS analogue) or **skipped** with a Rust-only / out-of-subset reason.
JS fixtures live under `fixtures/flux_*.js` and are driven by `tests/flux.rs`.
Porting rules, semantic mapping, and checker tradeoffs are in
[flux-rs porting rules](flux-rs-porting.md). The fixtures are also the
playground catalog.

Counts: 320 pos/surface files, 189 neg/surface files, 2 pos/vec files. Every pos/surface and neg/surface filename appears below as ported or skipped. The deferred-portable list is empty.

## First wave (ported)

| flux-rs source | JS fixture | Notes |
|---|---|---|
| `tests/tests/pos/surface/index00.rs` | `flux_assert_index_{positive,negative}.js` | boolean[true] assert + five/incr singletons |
| `tests/tests/pos/surface/test00.rs` | `flux_inc_dec_positive.js` | inc/dec pre-post |
| `tests/tests/pos/surface/test05.rs` | `flux_inc_dec_positive.js` | same inc family as test00 |
| `tests/tests/pos/surface/test06.rs` | `flux_double_positive.js` | double x+x with 0<x |
| `tests/tests/pos/surface/rvec00.rs` | `flux_rvec_literal_positive.js` | empty length 0 + [0,1] index; dropped rvec![e;n] repeat |
| `tests/tests/pos/surface/test02.rs` | `flux_rvec_push_get_positive.js` | push twice then [i]; dropped &mut |
| `tests/tests/pos/surface/fib_loop.rs` | `flux_fib_loop_positive.js` | while k>2 fib |
| `tests/tests/pos/surface/loop01.rs` | `flux_loop01_{positive,negative}.js` | count-up Houdini 0<=res |
| `tests/tests/pos/surface/loop00.rs` | `flux_countdown_{positive,negative}.js` | adapted: dropped toss()/i32::MAX, kept countdown to 0 |
| `tests/tests/pos/surface/scrape00.rs` | `flux_scrape_range_positive.js` | res == hi-lo via v==i-lo scrape |
| `tests/tests/pos/surface/if-then-else.rs` | `flux_min_{positive,negative}.js` | min; if-index became a predicate |
| `tests/tests/pos/surface/arg_syntax.rs` | `flux_arg_path_positive.js, flux_exists_bound_positive.js` | path + exists only; skipped &/[T;N]/slice |
| `tests/tests/neg/surface/test00.rs` | `flux_inc_dec_negative.js` | inc claiming v < x |
| `tests/tests/neg/surface/rvec00.rs` | `flux_rvec_oob_negative.js` | v[2] on length 2 |
| `tests/tests/neg/surface/test02.rs` | `flux_rvec_push_get_negative.js` | get past last push |
| `tests/tests/neg/surface/fib_loop.rs` | `flux_fib_loop_negative.js` | post 1<x fails at n=1 |
| `tests/tests/neg/surface/loop00.rs` | `flux_countdown_negative.js` | adapted: k not >= 0 so not [0] |
| `tests/tests/neg/surface/scrape00.rs` | `flux_scrape_range_negative.js` | adapted: wrong post hi-lo+1 (flux file disables scrape_quals) |
| `tests/tests/neg/surface/if-then-else.rs` | `flux_min_negative.js` | returns max |
| *(no flux neg for index00)* | `flux_assert_index_negative.js` | `assert(true)` with a false equality |

| `tests/tests/pos/surface/binop.rs` | `flux_logical_not_positive.js`, `flux_logical_or_index_positive.js` | logical `!` / `||` on bool indexes; bitwise/`/`/fn-ptr parts skipped |
| `tests/tests/neg/surface/binop.rs` | `flux_logical_not_negative.js` | `false || true` is not `boolean[false]` |
| `tests/tests/pos/surface/operators.rs` | `flux_unary_neg_positive.js`, `flux_neq_positive.js`, `flux_not_pred_positive.js`, `flux_bool_not_index_positive.js` | `-x`, `!==`, `!(x>0)`, `boolean[!x]`; skipped `/` |
| `tests/tests/neg/surface/operators.rs` | `flux_unary_neg_negative.js`, `flux_neq_negative.js`, `flux_not_pred_negative.js`, `flux_bool_not_index_negative.js` | same operators, failing calls |
| `tests/tests/pos/surface/literals00.rs` | `flux_literals_hex_positive.js` | `0xa` / `0o12` / `0b1010` in specs |
| `tests/tests/pos/surface/ex2_min_index_loop.rs` | `flux_min_index_{positive,negative}.js`, `flux_dense_param_{positive,negative}.js` | min-index walk (dropped struct `Bob`); `DenseArray[n]` param length |
| `tests/tests/pos/vec/vec_get.rs` | skipped | `Vec::get` returns `Option`, not RVec |
| `tests/tests/pos/vec/vec_macro.rs` | skipped | `vec!` macro |


## Skipped (Rust-only)

### opaque structs / relational lambdas

- `tests/tests/pos/surface/bin_rels.rs`
- `tests/tests/neg/surface/bin_rels.rs`

### bitwise-only / Rust integer overflow

- bitwise and shift functions in `binop.rs` (the logical subset is ported above)

### named const in signatures

- `tests/tests/pos/surface/const00.rs`
- `tests/tests/neg/surface/const00.rs`
- `tests/tests/pos/surface/const01.rs`
- `tests/tests/neg/surface/const01.rs`
- `tests/tests/pos/surface/const02.rs`
- `tests/tests/neg/surface/const02.rs`
- `tests/tests/pos/surface/const05.rs`
- `tests/tests/pos/surface/const06.rs`
- `tests/tests/pos/surface/const09.rs`

### shared refs / packing

- `tests/tests/pos/surface/constr00.rs`
- `tests/tests/neg/surface/constr00.rs`

### division

- `tests/tests/pos/surface/division.rs`
- `tests/tests/neg/surface/division.rs`
- `tests/tests/pos/surface/assert_terminator.rs`

### remainder / gcd

- `tests/tests/pos/surface/remainder.rs`
- `tests/tests/neg/surface/remainder.rs`
- `tests/tests/pos/surface/gcd.rs`

### already-covered IEEE Number

- `tests/tests/pos/surface/float00.rs`
- `tests/tests/pos/surface/float01.rs`
- `tests/tests/pos/surface/float02.rs`
- `tests/tests/neg/surface/float02.rs`
- `tests/tests/pos/surface/float03.rs`

### borrow join / &mut / strg

- `tests/tests/pos/surface/join00.rs`
- `tests/tests/neg/surface/join00.rs`
- `tests/tests/pos/surface/join01.rs`
- `tests/tests/neg/surface/join01.rs`
- `tests/tests/pos/surface/join02.rs`
- `tests/tests/pos/surface/join03.rs`
- `tests/tests/neg/surface/join03.rs`
- `tests/tests/pos/surface/join04.rs`
- `tests/tests/neg/surface/join04.rs`
- `tests/tests/pos/surface/test01.rs`
- `tests/tests/neg/surface/test01.rs`
- `tests/tests/pos/surface/test03.rs`
- `tests/tests/neg/surface/test03.rs`
- `tests/tests/pos/surface/read_loop.rs`
- `tests/tests/neg/surface/read_loop.rs`
- `tests/tests/pos/surface/read_ref.rs`
- `tests/tests/neg/surface/read_ref.rs`

### refined struct + Iterator

- `tests/tests/pos/surface/range.rs`
- `tests/tests/neg/surface/range.rs`

### integer division in binary search

- `tests/tests/pos/surface/bsearch.rs`
- `tests/tests/neg/surface/bsearch.rs`
- `tests/tests/pos/surface/bsearch1.rs`

### heap-field / length kvar (loop push)

- `tests/tests/pos/surface/scrape01.rs`


### abstract refinements / composite sorts

- `tests/tests/pos/surface/abstract_refinement_in_composite_sort.rs`

### type aliases / ealiases

- `tests/tests/pos/surface/alias00.rs`
- `tests/tests/pos/surface/alias03.rs`
- `tests/tests/pos/surface/alias04.rs`
- `tests/tests/pos/surface/alias05.rs`
- `tests/tests/pos/surface/ealias00.rs`
- `tests/tests/pos/surface/ealias01.rs`
- `tests/tests/pos/surface/ealias02.rs`
- `tests/tests/pos/surface/rust_alias.rs`
- `tests/tests/neg/surface/alias00.rs`
- `tests/tests/neg/surface/alias01.rs`
- `tests/tests/neg/surface/alias03.rs`
- `tests/tests/neg/surface/alias05.rs`
- `tests/tests/neg/surface/ealias00.rs`
- `tests/tests/neg/surface/ealias01.rs`
- `tests/tests/neg/surface/ealias02.rs`
- `tests/tests/neg/surface/rust_alias.rs`

### as-casts

- `tests/tests/pos/surface/as00.rs`
- `tests/tests/neg/surface/as00.rs`

### associated refinements / traits

- `tests/tests/pos/surface/assoc_reft00.rs`
- `tests/tests/pos/surface/assoc_reft01.rs`
- `tests/tests/pos/surface/assoc_reft04.rs`
- `tests/tests/pos/surface/assoc_reft05.rs`
- `tests/tests/pos/surface/assoc_reft07.rs`
- `tests/tests/pos/surface/assoc_reft08.rs`
- `tests/tests/pos/surface/assoc_reft09.rs`
- `tests/tests/pos/surface/assoc_reft10.rs`
- `tests/tests/pos/surface/assoc_reft11.rs`
- `tests/tests/pos/surface/final_assoc_reft00.rs`
- `tests/tests/pos/surface/final_assoc_reft01.rs`
- `tests/tests/neg/surface/assoc_reft00.rs`
- `tests/tests/neg/surface/assoc_reft01.rs`
- `tests/tests/neg/surface/assoc_reft02.rs`
- `tests/tests/neg/surface/assoc_reft03.rs`
- `tests/tests/neg/surface/assoc_reft05.rs`
- `tests/tests/neg/surface/assoc_reft07.rs`
- `tests/tests/neg/surface/assoc_reft08.rs`
- `tests/tests/neg/surface/assoc_reft11.rs`

### associated types / traits

- `tests/tests/pos/surface/associated_type00.rs`
- `tests/tests/pos/surface/associated_type01.rs`
- `tests/tests/pos/surface/associated_type02.rs`
- `tests/tests/neg/surface/associated_type02.rs`

### enums

- `tests/tests/pos/surface/assume_invariant00.rs`
- `tests/tests/pos/surface/explicit_variant_val.rs`
- `tests/tests/pos/surface/int_bounds_invariants.rs`
- `tests/tests/pos/surface/invariant-subtyping.rs`
- `tests/tests/pos/surface/invariant_with_const_generic.rs`
- `tests/tests/pos/surface/restrictable_variants.rs`
- `tests/tests/neg/surface/assume_invariant00.rs`
- `tests/tests/neg/surface/int_bounds_invariants.rs`

### flux assume / invariants

- `tests/tests/pos/surface/assume_parametric00.rs`
- `tests/tests/neg/surface/assume_parametric00.rs`

### async / Rust futures

- `tests/tests/pos/surface/async00.rs`
- `tests/tests/pos/surface/async01.rs`
- `tests/tests/neg/surface/async00.rs`
- `tests/tests/neg/surface/async01.rs`

### trait auto-strong

- `tests/tests/pos/surface/auto_strong00.rs`
- `tests/tests/pos/surface/auto_strong01.rs`

### traits

- `tests/tests/pos/surface/auto_strong_trait_00.rs`
- `tests/tests/pos/surface/higher_rank_trait00.rs`
- `tests/tests/pos/surface/rebase_trait_impl_generics00.rs`
- `tests/tests/pos/surface/refined_fn_in_trait.rs`
- `tests/tests/pos/surface/refined_fn_in_trait_01.rs`
- `tests/tests/pos/surface/refined_fn_in_trait_02.rs`
- `tests/tests/pos/surface/trait-subtyping01.rs`
- `tests/tests/pos/surface/trait-subtyping02.rs`
- `tests/tests/pos/surface/trait-subtyping03.rs`
- `tests/tests/pos/surface/trait-subtyping04.rs`
- `tests/tests/pos/surface/trait-subtyping06.rs`
- `tests/tests/pos/surface/trait-subtyping07.rs`
- `tests/tests/pos/surface/trait-subtyping08.rs`
- `tests/tests/pos/surface/trait00.rs`
- `tests/tests/pos/surface/trait01.rs`
- `tests/tests/pos/surface/trait01a.rs`
- `tests/tests/pos/surface/trait02.rs`
- `tests/tests/pos/surface/trait02_next.rs`
- `tests/tests/pos/surface/trait03.rs`
- `tests/tests/pos/surface/trait04.rs`
- `tests/tests/pos/surface/trait05.rs`
- `tests/tests/pos/surface/trait06.rs`
- `tests/tests/pos/surface/trait07.rs`
- `tests/tests/pos/surface/trait08.rs`
- `tests/tests/pos/surface/trait09.rs`
- `tests/tests/pos/surface/trait_alias00.rs`
- `tests/tests/neg/surface/refined_fn_in_trait.rs`
- `tests/tests/neg/surface/trait-subtyping01.rs`
- `tests/tests/neg/surface/trait-subtyping02.rs`
- `tests/tests/neg/surface/trait-subtyping03.rs`
- `tests/tests/neg/surface/trait-subtyping04.rs`
- `tests/tests/neg/surface/trait-subtyping05.rs`
- `tests/tests/neg/surface/trait-subtyping06.rs`
- `tests/tests/neg/surface/trait-subtyping07.rs`
- `tests/tests/neg/surface/trait-subtyping08.rs`
- `tests/tests/neg/surface/trait01.rs`
- `tests/tests/neg/surface/trait01a.rs`
- `tests/tests/neg/surface/trait02.rs`
- `tests/tests/neg/surface/trait02a.rs`
- `tests/tests/neg/surface/trait03.rs`

### compiletest auxiliary

- `tests/tests/pos/surface/auxiliary/flux_mod_children_aux.rs`

### RVec copy + more

- `tests/tests/pos/surface/bcopy.rs`
- `tests/tests/neg/surface/bcopy.rs`

### Rust overflow checking

- `tests/tests/pos/surface/binop_overflow.rs`
- `tests/tests/pos/surface/check_overflow00.rs`
- `tests/tests/pos/surface/check_overflow01.rs`
- `tests/tests/pos/surface/check_overflow03.rs`
- `tests/tests/pos/surface/check_overflow05.rs`
- `tests/tests/pos/surface/kmp_overflow.rs`
- `tests/tests/pos/surface/unop_overflow.rs`
- `tests/tests/neg/surface/binop_overflow.rs`
- `tests/tests/neg/surface/check_overflow00.rs`
- `tests/tests/neg/surface/check_overflow01.rs`
- `tests/tests/neg/surface/check_overflow02.rs`
- `tests/tests/neg/surface/check_overflow03.rs`
- `tests/tests/neg/surface/check_underflow00.rs`
- `tests/tests/neg/surface/unop_overflow.rs`

### bitvector sort

- `tests/tests/pos/surface/bitvec02.rs`
- `tests/tests/neg/surface/bitvec02.rs`

### borrows / lifetimes

- `tests/tests/pos/surface/borrow00.rs`

### quantifiers

- `tests/tests/pos/surface/bounded_quant00.rs`
- `tests/tests/neg/surface/bounded_quant00.rs`

### Box / ownership

- `tests/tests/pos/surface/box00.rs`
- `tests/tests/pos/surface/box01.rs`
- `tests/tests/pos/surface/box02.rs`
- `tests/tests/pos/surface/box03.rs`
- `tests/tests/neg/surface/box00.rs`
- `tests/tests/neg/surface/box01.rs`
- `tests/tests/neg/surface/box02.rs`
- `tests/tests/neg/surface/box03.rs`

### char sort

- `tests/tests/pos/surface/char00.rs`
- `tests/tests/pos/surface/char01.rs`
- `tests/tests/pos/surface/char02.rs`
- `tests/tests/pos/surface/char03.rs`

### closures

- `tests/tests/pos/surface/closure00.rs`
- `tests/tests/pos/surface/closure02.rs`
- `tests/tests/pos/surface/closure03.rs`
- `tests/tests/pos/surface/closure04.rs`
- `tests/tests/pos/surface/closure05.rs`
- `tests/tests/pos/surface/closure06.rs`
- `tests/tests/pos/surface/closure07.rs`
- `tests/tests/pos/surface/closure08.rs`
- `tests/tests/pos/surface/closure09.rs`
- `tests/tests/pos/surface/closure09_exi.rs`
- `tests/tests/pos/surface/closure10.rs`
- `tests/tests/pos/surface/closure13.rs`
- `tests/tests/pos/surface/closure15.rs`
- `tests/tests/neg/surface/closure00.rs`
- `tests/tests/neg/surface/closure02.rs`
- `tests/tests/neg/surface/closure04.rs`
- `tests/tests/neg/surface/closure05.rs`
- `tests/tests/neg/surface/closure06.rs`
- `tests/tests/neg/surface/closure07.rs`
- `tests/tests/neg/surface/closure08.rs`
- `tests/tests/neg/surface/closure09.rs`
- `tests/tests/neg/surface/closure10.rs`

### string / str specs

- `tests/tests/pos/surface/constr01.rs`
- `tests/tests/pos/surface/constr02.rs`
- `tests/tests/pos/surface/constr03.rs`
- `tests/tests/pos/surface/str01.rs`
- `tests/tests/neg/surface/constr01.rs`
- `tests/tests/neg/surface/constr02.rs`
- `tests/tests/neg/surface/constr03.rs`
- `tests/tests/neg/surface/str01.rs`
- `tests/tests/neg/surface/str02.rs`
- `tests/tests/neg/surface/str03.rs`

### structs

- `tests/tests/pos/surface/date.rs`
- `tests/tests/neg/surface/date.rs`

### default trait

- `tests/tests/pos/surface/default00.rs`
- `tests/tests/neg/surface/default00.rs`

### join / ghost

- `tests/tests/pos/surface/dummy_join00.rs`

### existentials

- `tests/tests/pos/surface/empty_exists.rs`
- `tests/tests/pos/surface/general_exists00.rs`
- `tests/tests/pos/surface/output-exists00.rs`
- `tests/tests/pos/surface/output-exists01.rs`
- `tests/tests/neg/surface/general_exists00.rs`
- `tests/tests/neg/surface/output-exists00.rs`
- `tests/tests/neg/surface/output-exists01.rs`

### Result ADT

- `tests/tests/pos/surface/err-res.rs`
- `tests/tests/pos/surface/result00.rs`
- `tests/tests/neg/surface/result00.rs`

### MIR / ghost stmts

- `tests/tests/pos/surface/exit-basic-block-with-ghost-stmts.rs`

### extern specs

- `tests/tests/pos/surface/extern_function00.rs`
- `tests/tests/pos/surface/extern_static00.rs`

### large / RVec

- `tests/tests/pos/surface/fft.rs`
- `tests/tests/neg/surface/fft.rs`

### unit params

- `tests/tests/pos/surface/filter_unit_params.rs`

### fixpoint parser

- `tests/tests/pos/surface/fixpoint_eq_precedence.rs`

### fn defs as values

- `tests/tests/pos/surface/fndef00.rs`
- `tests/tests/pos/surface/fndef01.rs`
- `tests/tests/pos/surface/fndef02.rs`
- `tests/tests/neg/surface/fndef00.rs`
- `tests/tests/neg/surface/fndef01.rs`
- `tests/tests/neg/surface/fndef02.rs`

### raw pointers

- `tests/tests/pos/surface/fnptr00.rs`
- `tests/tests/pos/surface/fnptr01.rs`
- `tests/tests/pos/surface/local_ptr00.rs`
- `tests/tests/pos/surface/ptr00.rs`
- `tests/tests/pos/surface/ptr01.rs`
- `tests/tests/pos/surface/ptr02.rs`
- `tests/tests/pos/surface/raw_ptr_field_mut_ref.rs`
- `tests/tests/pos/surface/raw_ptr_rvalue.rs`
- `tests/tests/pos/surface/reifyFnPtr00.rs`
- `tests/tests/neg/surface/local_ptr00.rs`
- `tests/tests/neg/surface/reifyFnPtr00.rs`

### fold/unfold

- `tests/tests/pos/surface/fold00.rs`

### quantifiers beyond unary p

- `tests/tests/pos/surface/forall01.rs`
- `tests/tests/pos/surface/forall02.rs`
- `tests/tests/neg/surface/forall01.rs`
- `tests/tests/neg/surface/forall02.rs`

### extern / foreign types

- `tests/tests/pos/surface/foreign_type00.rs`

### join

- `tests/tests/pos/surface/generalized_join.rs`

### GhostCell

- `tests/tests/pos/surface/ghostcell00.rs`
- `tests/tests/neg/surface/ghostcell00.rs`

### RVec + structs

- `tests/tests/pos/surface/heapsort.rs`

### flux hide

- `tests/tests/pos/surface/hide00.rs`
- `tests/tests/neg/surface/hide00.rs`

### flux ignore attribute

- `tests/tests/pos/surface/ignore00.rs`
- `tests/tests/neg/surface/ignore00.rs`
- `tests/tests/neg/surface/ignore01.rs`
- `tests/tests/neg/surface/ignore02.rs`

### impl blocks / traits

- `tests/tests/pos/surface/impl00.rs`
- `tests/tests/pos/surface/impl01.rs`
- `tests/tests/pos/surface/impl02.rs`
- `tests/tests/pos/surface/impl03.rs`
- `tests/tests/neg/surface/impl00.rs`
- `tests/tests/neg/surface/impl01.rs`
- `tests/tests/neg/surface/impl02.rs`
- `tests/tests/neg/surface/impl03.rs`

### intrinsics

- `tests/tests/pos/surface/intrinsic_assume00.rs`

### rustc issue reproduction

- `tests/tests/pos/surface/issue-1037.rs`
- `tests/tests/pos/surface/issue-1109.rs`
- `tests/tests/pos/surface/issue-1143.rs`
- `tests/tests/pos/surface/issue-1359.rs`
- `tests/tests/pos/surface/issue-141.rs`
- `tests/tests/pos/surface/issue-1427.rs`
- `tests/tests/pos/surface/issue-1449-simp.rs`
- `tests/tests/pos/surface/issue-1564.rs`
- `tests/tests/pos/surface/issue-185.rs`
- `tests/tests/pos/surface/issue-220.rs`
- `tests/tests/pos/surface/issue-231.rs`
- `tests/tests/pos/surface/issue-258.rs`
- `tests/tests/pos/surface/issue-271.rs`
- `tests/tests/pos/surface/issue-283.rs`
- `tests/tests/pos/surface/issue-299.rs`
- `tests/tests/pos/surface/issue-332.rs`
- `tests/tests/pos/surface/issue-334.rs`
- `tests/tests/pos/surface/issue-431.rs`
- `tests/tests/pos/surface/issue-529.rs`
- `tests/tests/pos/surface/issue-555.rs`
- `tests/tests/pos/surface/issue-569.rs`
- `tests/tests/pos/surface/issue-654.rs`
- `tests/tests/pos/surface/issue-658.rs`
- `tests/tests/pos/surface/issue-662.rs`
- `tests/tests/pos/surface/issue-672.rs`
- `tests/tests/pos/surface/issue-687.rs`
- `tests/tests/pos/surface/issue-698.rs`
- `tests/tests/pos/surface/issue-703.rs`
- `tests/tests/pos/surface/issue-706.rs`
- `tests/tests/pos/surface/issue-711.rs`
- `tests/tests/pos/surface/issue-725.rs`
- `tests/tests/pos/surface/issue-73.rs`
- `tests/tests/pos/surface/issue-742.rs`
- `tests/tests/pos/surface/issue-743.rs`
- `tests/tests/pos/surface/issue-790.rs`
- `tests/tests/pos/surface/issue-792.rs`
- `tests/tests/pos/surface/issue-809.rs`
- `tests/tests/pos/surface/issue-829.rs`
- `tests/tests/pos/surface/issue-829b.rs`
- `tests/tests/pos/surface/issue-837.rs`
- `tests/tests/pos/surface/issue-841.rs`
- `tests/tests/pos/surface/issue-899b.rs`
- `tests/tests/pos/surface/issue-977.rs`
- `tests/tests/pos/surface/issue-983.rs`
- `tests/tests/neg/surface/issue-141.rs`
- `tests/tests/neg/surface/issue-158.rs`
- `tests/tests/neg/surface/issue-258.rs`
- `tests/tests/neg/surface/issue-271.rs`
- `tests/tests/neg/surface/issue-299.rs`
- `tests/tests/neg/surface/issue-431.rs`
- `tests/tests/neg/surface/issue-588.rs`
- `tests/tests/neg/surface/issue-767.rs`

### iterators

- `tests/tests/pos/surface/iter00.rs`
- `tests/tests/pos/surface/iter01.rs`
- `tests/tests/neg/surface/iter00.rs`
- `tests/tests/neg/surface/iter01.rs`

### large / RVec+structs

- `tests/tests/pos/surface/kmeans.rs`
- `tests/tests/neg/surface/kmeans.rs`

### large / overflow

- `tests/tests/pos/surface/kmp.rs`

### RVec shuffle

- `tests/tests/pos/surface/knuth_shuffle.rs`

### let in specs

- `tests/tests/pos/surface/let-exprs00.rs`
- `tests/tests/neg/surface/let-exprs00.rs`

### local qualifiers

- `tests/tests/pos/surface/local_qual00.rs`
- `tests/tests/neg/surface/local_qual00.rs`

### abstract refinements in loops

- `tests/tests/pos/surface/loop_abstract_refinement.rs`

### Rust macros

- `tests/tests/pos/surface/macro-expansion.rs`
- `tests/tests/pos/surface/macro-expansion01.rs`
- `tests/tests/neg/surface/macro-expansion01.rs`

### unsizing

- `tests/tests/pos/surface/mut-ref-array-unsize.rs`
- `tests/tests/pos/surface/unsize00.rs`
- `tests/tests/neg/surface/unsize00.rs`

### Rust mut/strg

- `tests/tests/pos/surface/mut_as_strg00.rs`
- `tests/tests/pos/surface/mut_hack00.rs`
- `tests/tests/pos/surface/unblock_mut_mut_ref.rs`
- `tests/tests/neg/surface/mut_as_strg00.rs`
- `tests/tests/neg/surface/mut_hack00.rs`

### kvars / binders

- `tests/tests/pos/surface/nested_binders_kvar.rs`

### Rust panics

- `tests/tests/pos/surface/no_panic00.rs`
- `tests/tests/pos/surface/no_panic02.rs`
- `tests/tests/pos/surface/no_panic03.rs`
- `tests/tests/pos/surface/no_panic05.rs`
- `tests/tests/pos/surface/no_panic06.rs`
- `tests/tests/pos/surface/no_panic07.rs`
- `tests/tests/pos/surface/no_panic08.rs`
- `tests/tests/pos/surface/no_panic09.rs`
- `tests/tests/pos/surface/panic00.rs`
- `tests/tests/neg/surface/no_panic00.rs`
- `tests/tests/neg/surface/no_panic01.rs`
- `tests/tests/neg/surface/no_panic02.rs`
- `tests/tests/neg/surface/no_panic03.rs`
- `tests/tests/neg/surface/no_panic04.rs`
- `tests/tests/neg/surface/no_panic06.rs`
- `tests/tests/neg/surface/no_panic07.rs`

### NonZeroUsize

- `tests/tests/pos/surface/non_zero.rs`

### Rust numeric consts

- `tests/tests/pos/surface/num_consts.rs`
- `tests/tests/neg/surface/num_consts.rs`

### optimization blowup

- `tests/tests/pos/surface/opt_blowup.rs`

### existential pack

- `tests/tests/pos/surface/pack00.rs`

### mut refs

- `tests/tests/pos/surface/param-under-mut-ref.rs`

### tuples

- `tests/tests/pos/surface/partially_uninit_tuple.rs`
- `tests/tests/pos/surface/tuple00.rs`
- `tests/tests/pos/surface/tuple_sorts00.rs`
- `tests/tests/neg/surface/tuple00.rs`
- `tests/tests/neg/surface/tuple_sorts01.rs`

### paper bug / rust-specific

- `tests/tests/pos/surface/pldi23ae-reviewerb-bug.rs`

### user qualifiers

- `tests/tests/pos/surface/polymorphic_qualifier.rs`

### primop let

- `tests/tests/pos/surface/primop_prop_let.rs`

### const promotion

- `tests/tests/pos/surface/promotion02.rs`

### real sort

- `tests/tests/pos/surface/real00.rs`
- `tests/tests/pos/surface/real01.rs`
- `tests/tests/pos/surface/real02.rs`
- `tests/tests/neg/surface/real00.rs`
- `tests/tests/neg/surface/real01.rs`
- `tests/tests/neg/surface/real02.rs`
- `tests/tests/neg/surface/real03.rs`

### references / RefCell

- `tests/tests/pos/surface/ref_cell00.rs`
- `tests/tests/pos/surface/ref_param.rs`
- `tests/tests/neg/surface/ref_cell00.rs`
- `tests/tests/neg/surface/ref_param.rs`

### refined type variables

- `tests/tests/pos/surface/refined_type_var00.rs`
- `tests/tests/pos/surface/refined_type_var01.rs`
- `tests/tests/neg/surface/refined_type_var00.rs`

### Rust name resolution

- `tests/tests/pos/surface/resolver00.rs`
- `tests/tests/pos/surface/resolver01.rs`
- `tests/tests/pos/surface/resolver02.rs`
- `tests/tests/pos/surface/resolver03.rs`
- `tests/tests/pos/surface/resolver04.rs`
- `tests/tests/pos/surface/resolver05.rs`
- `tests/tests/pos/surface/resolver06.rs`
- `tests/tests/pos/surface/resolver07.rs`
- `tests/tests/pos/surface/resolver08.rs`
- `tests/tests/pos/surface/resolver09.rs`
- `tests/tests/pos/surface/resolver10.rs`
- `tests/tests/pos/surface/resolver11.rs`
- `tests/tests/pos/surface/resolver12.rs`
- `tests/tests/pos/surface/resolver14.rs`
- `tests/tests/neg/surface/resolver00.rs`
- `tests/tests/neg/surface/resolver13.rs`

### refined matrices

- `tests/tests/pos/surface/rmat.rs`
- `tests/tests/pos/surface/rmat00.rs`
- `tests/tests/neg/surface/rmat.rs`
- `tests/tests/neg/surface/rmat00.rs`

### refined sets

- `tests/tests/pos/surface/rset00.rs`
- `tests/tests/pos/surface/rset01.rs`
- `tests/tests/pos/surface/rset04a.rs`
- `tests/tests/neg/surface/rset00.rs`
- `tests/tests/neg/surface/rset01.rs`
- `tests/tests/neg/surface/rset03.rs`

### refined slices

- `tests/tests/pos/surface/rslice00.rs`
- `tests/tests/neg/surface/rslice00.rs`

### Rust scoping

- `tests/tests/pos/surface/scope00.rs`

### Self type alias

- `tests/tests/pos/surface/self_ty_alias00.rs`
- `tests/tests/pos/surface/self_ty_alias01.rs`
- `tests/tests/pos/surface/self_ty_alias02.rs`

### compiletest should-fail

- `tests/tests/pos/surface/should_fail.rs`
- `tests/tests/neg/surface/should_fail.rs`

### large

- `tests/tests/pos/surface/simplex.rs`
- `tests/tests/neg/surface/simplex.rs`

### Rust slices

- `tests/tests/pos/surface/slice00.rs`
- `tests/tests/pos/surface/slice01.rs`
- `tests/tests/neg/surface/slice00.rs`
- `tests/tests/neg/surface/slice01.rs`

### SMT define-fun / rust const

- `tests/tests/pos/surface/smt_define_fun_with_rust_const.rs`

### sort inference edge

- `tests/tests/pos/surface/sort_inference00.rs`
- `tests/tests/neg/surface/sort_inference00.rs`

### static items

- `tests/tests/pos/surface/static_spec00.rs`
- `tests/tests/neg/surface/static_spec00.rs`

### strg / &mut ensures

- `tests/tests/pos/surface/strg_to_mut.rs`
- `tests/tests/neg/surface/strg_to_mut.rs`

### refined structs

- `tests/tests/pos/surface/struct_invariant.rs`
- `tests/tests/neg/surface/struct_invariant00.rs`

### synthetic params

- `tests/tests/pos/surface/synthetic-param.rs`
- `tests/tests/neg/surface/synthetic-param.rs`

### where clauses

- `tests/tests/pos/surface/test01_where.rs`
- `tests/tests/neg/surface/test01_where.rs`

### duplicate of test00-style / check

- `tests/tests/pos/surface/test04.rs`
- `tests/tests/neg/surface/test04.rs`

### cast / to_int

- `tests/tests/pos/surface/to_int01.rs`
- `tests/tests/neg/surface/to_int01.rs`

### structs / lists

- `tests/tests/pos/surface/too_many_linked_lists.rs`
- `tests/tests/neg/surface/too_many_linked_lists.rs`

### Rust derive

- `tests/tests/pos/surface/trusted_derive00.rs`
- `tests/tests/neg/surface/trusted_derive00.rs`

### lifetimes

- `tests/tests/pos/surface/type-outlives.rs`

### type holes

- `tests/tests/pos/surface/type_holes00.rs`
- `tests/tests/pos/surface/type_holes01.rs`
- `tests/tests/pos/surface/type_holes02.rs`

### uninterpreted functions

- `tests/tests/pos/surface/uif00.rs`
- `tests/tests/pos/surface/uif01.rs`
- `tests/tests/pos/surface/uif02.rs`
- `tests/tests/neg/surface/uif00.rs`
- `tests/tests/neg/surface/uif01.rs`
- `tests/tests/neg/surface/uif02.rs`

### union sorts

- `tests/tests/pos/surface/union_sort_resolution00.rs`

### unpack

- `tests/tests/pos/surface/unpack-in-subtyping.rs`

### Rust-only or outside the JS subset (structs/enums/traits/borrows/overflow/etc.)

- `tests/tests/neg/surface/arg_syntax.rs`
- `tests/tests/neg/surface/assume00.rs`
- `tests/tests/neg/surface/const03.rs`
- `tests/tests/neg/surface/const04.rs`
- `tests/tests/neg/surface/test05.rs`
- `tests/tests/neg/surface/test06.rs`

### const requires

- `tests/tests/neg/surface/const_requires.rs`

### holes

- `tests/tests/neg/surface/hole00.rs`

### refined maps

- `tests/tests/neg/surface/maps00.rs`
- `tests/tests/neg/surface/maps01.rs`

### Option ADT

- `tests/tests/neg/surface/option00.rs`

### pledge / strg

- `tests/tests/neg/surface/pledge00.rs`

### reflect

- `tests/tests/neg/surface/reflect00.rs`

### std Vec, not RVec

- `tests/tests/neg/surface/vec01.rs`

