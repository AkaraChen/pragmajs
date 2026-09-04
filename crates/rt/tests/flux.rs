use pragma_rt::{checker, parser, prelude::Environment, runtime, syntax::Annotation, transpiler};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn flux_fixtures(suffix: &str) -> Vec<PathBuf> {
    let fixtures_dir = fixture_path("");
    let mut paths: Vec<_> = fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixtures_dir.display()))
        .map(|entry| {
            entry
                .expect("failed to read fixture directory entry")
                .path()
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("flux_") && name.ends_with(suffix))
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no flux fixtures matched *{suffix}");
    paths
}

fn parse_fixture(path: &Path) -> (String, Vec<Annotation>) {
    let file_name = path.display().to_string();
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {file_name}: {error}"));
    let parsed = parser::parse_file(&source, &file_name)
        .unwrap_or_else(|error| panic!("failed to parse {file_name}: {error}"));
    (source, parsed.annotations)
}

fn assert_statically_valid_and_runs(path: &Path) {
    let file_name = path.display().to_string();
    let (source, annotations) = parse_fixture(path);

    let errors = checker::check_source_with_environment(
        &source,
        &file_name,
        &annotations,
        Environment::Auto,
    );
    assert!(
        errors.is_empty(),
        "expected {file_name} to verify statically, got:\n{errors:#?}"
    );

    let transformed = transpiler::transpile(&source, &file_name, &annotations)
        .unwrap_or_else(|error| panic!("failed to transpile {file_name}: {error}"));
    assert!(
        transformed.contains("__rt.assert"),
        "transpiled {file_name} did not preserve runtime refinement assertions"
    );

    let executable = format!("{}\n\n{transformed}", runtime::runtime_block());
    let output = Command::new("node")
        .args(["-e", &executable])
        .output()
        .unwrap_or_else(|error| panic!("failed to execute Node.js for {file_name}: {error}"));
    assert!(
        output.status.success(),
        "Node.js execution failed for {file_name}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn flux_positive_fixtures_verify_and_run_without_assertion_failures() {
    for path in flux_fixtures("_positive.js") {
        assert_statically_valid_and_runs(&path);
    }
}

#[test]
fn flux_negative_fixtures_are_rejected_statically() {
    for path in flux_fixtures("_negative.js") {
        let file_name = path.display().to_string();
        let (source, annotations) = parse_fixture(&path);
        let errors = checker::check_source_with_environment(
            &source,
            &file_name,
            &annotations,
            Environment::Auto,
        );
        assert!(
            !errors.is_empty(),
            "expected {file_name} to be rejected statically"
        );
        assert!(
            errors
                .iter()
                .all(|error| !error.message.contains("Z3 returned unknown")),
            "{file_name} was rejected only because the solver returned unknown: {errors:#?}"
        );
    }
}

#[test]
fn soundness_regressions_have_definite_diagnostics() {
    let cases = [
        ("flux_polymorphism_vacuity_negative.js", "Return value"),
        ("flux_polymorphism_body_negative.js", "Return value"),
        ("flux_float_rounding_negative.js", "Return value"),
        ("flux_predicate_kind_negative.js", "incompatible base types"),
        (
            "flux_parameter_shadow_negative.js",
            "shadows refined parameter",
        ),
        (
            "flux_uninitialized_local_negative.js",
            "requires an initializer",
        ),
        (
            "flux_destructuring_negative.js",
            "Destructuring declarations",
        ),
        ("flux_var_declaration_negative.js", "Only let and const"),
        ("flux_const_assignment_negative.js", "immutable binding"),
        (
            "flux_callee_shadow_negative.js",
            "shadows a refined function signature",
        ),
        ("flux_async_function_negative.js", "Async and generator"),
        ("flux_generator_function_negative.js", "Async and generator"),
        ("flux_ill_typed_predicate_negative.js", "boolean operands"),
        ("flux_void_value_negative.js", "boolean operands"),
        ("flux_runtime_binding_negative.js", "reserved"),
        ("flux_default_parameter_negative.js", "Default parameters"),
        (
            "flux_nested_variable_annotation_negative.js",
            "outside a statically checked scope",
        ),
        (
            "flux_orphan_parameter_annotation_negative.js",
            "requires a function signature",
        ),
        ("flux_unused_predicate_negative.js", "must occur"),
        ("flux_tdz_negative.js", "before its declaration"),
        ("flux_console_spread_negative.js", "Spread arguments"),
        ("flux_prelude_shadow_negative.js", "reserved"),
        (
            "flux_index_singleton_negative.js",
            "does not match its index",
        ),
        ("flux_index_param_negative.js", "does not match its index"),
        (
            "flux_dense_oob_negative.js",
            "outside the collection bounds",
        ),
        ("flux_dense_empty_pop_negative.js", "non-empty dense array"),
        (
            "flux_loop_factorial_negative.js",
            "does not satisfy its refinement",
        ),
        (
            "flux_loop_dense_index_negative.js",
            "outside the collection bounds",
        ),
        (
            "flux_loop_dense_empty_pop_negative.js",
            "non-empty dense array",
        ),
        (
            "flux_nan_alias_negative.js",
            "does not satisfy its refinement",
        ),
        ("flux_assert_index_negative.js", "does not match its index"),
        (
            "flux_inc_dec_negative.js",
            "does not satisfy its refinement",
        ),
        ("flux_rvec_oob_negative.js", "outside the collection bounds"),
        (
            "flux_rvec_push_get_negative.js",
            "outside the collection bounds",
        ),
        (
            "flux_fib_loop_negative.js",
            "does not satisfy its refinement",
        ),
        ("flux_loop01_negative.js", "does not satisfy its refinement"),
        ("flux_countdown_negative.js", "does not match its index"),
        ("flux_scrape_range_negative.js", "does not match its index"),
        ("flux_min_negative.js", "does not satisfy its refinement"),
        ("flux_logical_not_negative.js", "does not match its index"),
        (
            "flux_unary_neg_negative.js",
            "does not satisfy its refinement",
        ),
        ("flux_neq_negative.js", "does not satisfy its refinement"),
        (
            "flux_not_pred_negative.js",
            "does not satisfy its refinement",
        ),
        (
            "flux_bool_not_index_negative.js",
            "does not match its index",
        ),
        (
            "flux_min_index_negative.js",
            "does not satisfy its refinement",
        ),
        ("flux_dense_param_negative.js", "does not match its index"),
    ];

    for (fixture, expected) in cases {
        let path = fixture_path(fixture);
        let file_name = path.display().to_string();
        let (source, annotations) = parse_fixture(&path);
        let errors = checker::check_source_with_environment(
            &source,
            &file_name,
            &annotations,
            Environment::Auto,
        );
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected {file_name} to report {expected:?}, got {errors:#?}"
        );
    }
}

#[test]
fn flux_rs_triage_lists_deferred_neg_surface_twins() {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/flux-rs-test-triage.md"),
    )
    .expect("failed to read flux-rs triage note");
    for name in [
        "bin_rels.rs",
        "binop.rs",
        "bsearch.rs",
        "const00.rs",
        "const01.rs",
        "const02.rs",
        "constr00.rs",
        "division.rs",
        "float02.rs",
        "join00.rs",
        "join01.rs",
        "join03.rs",
        "join04.rs",
        "operators.rs",
        "range.rs",
        "read_loop.rs",
        "read_ref.rs",
        "remainder.rs",
        "test01.rs",
        "test03.rs",
    ] {
        let needle = format!("tests/tests/neg/surface/{name}");
        assert!(text.contains(&needle), "triage note is missing {needle}");
    }
    assert!(
        !text.contains("## Deferred portable"),
        "triage note still has a Deferred portable table"
    );
}

#[test]
fn transpiler_preserves_parameter_return_and_variable_assertions_hygienically() {
    let core_path = fixture_path("flux_core_positive.js");
    let (core_source, core_annotations) = parse_fixture(&core_path);
    let core_output = transpiler::transpile(
        &core_source,
        &core_path.display().to_string(),
        &core_annotations,
    )
    .unwrap();
    assert_eq!(core_output.matches("__rt.assert").count(), 10);
    assert!(core_output.contains("parameter"));
    assert!(core_output.contains("return value"));
    assert!(core_output.contains("variable"));

    let hygiene_path = fixture_path("flux_hygiene_positive.js");
    let (hygiene_source, hygiene_annotations) = parse_fixture(&hygiene_path);
    let hygiene_output = transpiler::transpile(
        &hygiene_source,
        &hygiene_path.display().to_string(),
        &hygiene_annotations,
    )
    .unwrap();
    assert_eq!(hygiene_output.matches("__rt.assert").count(), 3);
    assert!(hygiene_output.contains("__rt_return_1"));
    assert!(hygiene_output.contains("__rt_v_1"));

    let unicode_path = fixture_path("flux_unicode_hygiene_positive.js");
    let (unicode_source, unicode_annotations) = parse_fixture(&unicode_path);
    let unicode_output = transpiler::transpile(
        &unicode_source,
        &unicode_path.display().to_string(),
        &unicode_annotations,
    )
    .unwrap();
    assert_eq!(unicode_output.matches("__rt.assert").count(), 2);
    assert!(unicode_output.contains("__rt_return_1"));
    assert!(unicode_output.contains("__rt_v_1"));
}

#[test]
fn existing_sqrt_fixture_verifies_and_runs() {
    assert_statically_valid_and_runs(&fixture_path("sqrt.js"));
}
