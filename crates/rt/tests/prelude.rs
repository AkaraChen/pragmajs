use pragma_rt::{
    checker, parser,
    prelude::Environment,
    runtime, transpiler,
    verifier::{RtAblation, RtFeatures},
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{self, Command},
};

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("prelude")
        .join(relative)
}

fn parse_fixture(relative: &str) -> (String, Vec<pragma_rt::syntax::Annotation>) {
    let path = fixture_path(relative);
    let file_name = path.display().to_string();
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {file_name}: {error}"));
    let parsed = parser::parse_file(&source, &file_name)
        .unwrap_or_else(|error| panic!("failed to parse {file_name}: {error}"));
    (source, parsed.annotations)
}

fn check_fixture(relative: &str, environment: Environment) -> Vec<pragma_rt::syntax::RtError> {
    let path = fixture_path(relative);
    let file_name = path.display().to_string();
    let (source, annotations) = parse_fixture(relative);
    checker::check_source_with_environment(&source, &file_name, &annotations, environment)
}

fn diagnostics(errors: &[pragma_rt::syntax::RtError]) -> String {
    errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_fixture_valid(relative: &str, environment: Environment) {
    let errors = check_fixture(relative, environment);
    assert!(
        errors.is_empty(),
        "expected {relative} to pass for {environment}, got:\n{}",
        diagnostics(&errors)
    );
}

fn assert_fixture_rejected_with(relative: &str, environment: Environment, expected: &str) {
    let errors = check_fixture(relative, environment);
    let diagnostics = diagnostics(&errors);
    assert!(
        diagnostics.contains(expected),
        "expected {relative} under {environment} to report {expected:?}, got:\n{diagnostics}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("Z3 returned unknown")),
        "{relative} was rejected because the solver returned unknown:\n{diagnostics}"
    );
}

#[test]
fn bun_stop_heap_invalidation_has_an_ablation_witness() {
    let relative = "bun/bun_stop_heap_effect_negative.js";
    let path = fixture_path(relative);
    let file_name = path.display().to_string();
    let (source, annotations) = parse_fixture(relative);
    let baseline = checker::check_source_with_environment_and_features(
        &source,
        &file_name,
        &annotations,
        Environment::Bun,
        RtFeatures::default(),
    );
    assert!(
        diagnostics(&baseline).contains("requires a non-empty dense array"),
        "expected stop() to invalidate the prior dense-array length fact, got:\n{}",
        diagnostics(&baseline)
    );

    let ablated = checker::check_source_with_environment_and_features(
        &source,
        &file_name,
        &annotations,
        Environment::Bun,
        RtFeatures::default().without(RtAblation::HeapFactInvalidation),
    );
    assert!(
        ablated.is_empty(),
        "heap-invalidation ablation should expose the witness, got:\n{}",
        diagnostics(&ablated)
    );
}

fn environment_from_target(target: &str) -> Environment {
    match target {
        "auto" => Environment::Auto,
        "ecmascript" => Environment::Ecmascript,
        "browser" => Environment::Browser,
        "node" => Environment::Node,
        "deno" => Environment::Deno,
        "bun" => Environment::Bun,
        other => panic!("unknown target {other}"),
    }
}

#[test]
fn standard_library_positive_fixtures_verify_for_explicit_targets() {
    for (fixture, environment) in [
        ("common/array_positive.js", Environment::Ecmascript),
        ("node/node_positive.js", Environment::Node),
        ("deno/deno_positive.js", Environment::Deno),
        ("deno/compat_positive.js", Environment::Deno),
        ("bun/bun_positive.js", Environment::Bun),
        ("dom/dom_positive.js", Environment::Browser),
        ("dom/dom_inheritance_positive.js", Environment::Browser),
    ] {
        assert_fixture_valid(fixture, environment);
    }
}

#[test]
fn array_contracts_reject_wrong_callback_result_argument_and_bounds() {
    for (fixture, expected) in [
        ("common/array_alias_negative.js", "Base type mismatch"),
        (
            "common/array_argument_effect_negative.js",
            "Initializer for 'finalLength' does not satisfy its refinement",
        ),
        ("common/array_callback_negative.js", "Base type mismatch"),
        (
            "common/array_callback_precondition_negative.js",
            "Base type mismatch",
        ),
        (
            "common/array_callback_repeated_effect_negative.js",
            "Argument 1 to 'exactlyTwo' does not satisfy its refinement",
        ),
        ("common/array_includes_negative.js", "Base type mismatch"),
        (
            "common/array_length_negative.js",
            "Initializer for 'impossible' does not satisfy its refinement",
        ),
        (
            "common/array_oob_negative.js",
            "Indexed access may be outside the collection bounds",
        ),
    ] {
        assert_fixture_rejected_with(fixture, Environment::Ecmascript, expected);
    }
}

#[test]
fn curated_prelude_never_overrides_program_bindings_or_unsafe_runtime_inputs() {
    for (fixture, environment, expected) in [
        (
            "common/object_keys_nullish_negative.js",
            Environment::Ecmascript,
            "No standard-library method 'keys'",
        ),
        (
            "common/structured_clone_shadow_negative.js",
            Environment::Ecmascript,
            "Local function 'structuredClone' requires an explicit refinement contract",
        ),
        (
            "common/deferred_callback_capture_negative.js",
            Environment::Browser,
            "Argument 1 to 'requirePositive' does not satisfy its refinement",
        ),
        (
            "common/user_function_heap_effect_negative.js",
            Environment::Ecmascript,
            "Initializer for 'staleLength' does not satisfy its refinement",
        ),
        (
            "common/side_effect_import_negative.js",
            Environment::Ecmascript,
            "Initializer for 'pushed' does not satisfy its refinement",
        ),
        (
            "node/import_namespace_shadow_negative.js",
            Environment::Node,
            "No standard-library method 'sqrt'",
        ),
        (
            "node/import_alias_shadow_negative.js",
            Environment::Node,
            "No standard-library method 'cwd'",
        ),
        (
            "node/function_shadow_negative.js",
            Environment::Node,
            "No refinement signature for top-level function 'process'",
        ),
        (
            "node/missing_export_negative.js",
            Environment::Node,
            "No standard-library declaration for export 'definitelyMissing'",
        ),
        (
            "dom/append_child_negative.js",
            Environment::Browser,
            "Base type mismatch",
        ),
        (
            "dom/click_heap_effect_negative.js",
            Environment::Browser,
            "Initializer for 'count' does not satisfy its refinement",
        ),
        (
            "dom/click_scalar_effect_negative.js",
            Environment::Browser,
            "Initializer for 'observed' does not satisfy its refinement",
        ),
    ] {
        assert_fixture_rejected_with(fixture, environment, expected);
    }
}

#[test]
fn platform_contracts_reject_invalid_arguments_and_nullable_dom_access() {
    for (fixture, environment, expected) in [
        (
            "node/node_argument_negative.js",
            Environment::Node,
            "Base type mismatch",
        ),
        (
            "deno/deno_argument_negative.js",
            Environment::Deno,
            "expects 0 arguments, got 1",
        ),
        (
            "bun/bun_argument_negative.js",
            Environment::Bun,
            "Base type mismatch",
        ),
        (
            "bun/bun_serve_argument_negative.js",
            Environment::Bun,
            "Base type mismatch",
        ),
        (
            "dom/dom_nullable_negative.js",
            Environment::Browser,
            "No static property 'childElementCount'",
        ),
    ] {
        assert_fixture_rejected_with(fixture, environment, expected);
    }
}

#[test]
fn cli_target_selection_accepts_common_code_and_isolates_platform_globals() {
    for target in ["ecmascript", "browser", "node", "deno", "bun"] {
        assert_fixture_valid("common/array_positive.js", environment_from_target(target));
    }

    for (fixture, target) in [
        ("node/node_positive.js", "node"),
        ("deno/deno_positive.js", "deno"),
        ("deno/compat_positive.js", "deno"),
        ("bun/bun_positive.js", "bun"),
        ("dom/dom_positive.js", "browser"),
        ("dom/dom_inheritance_positive.js", "browser"),
    ] {
        assert_fixture_valid(fixture, environment_from_target(target));
    }

    for (fixture, wrong_target, expected) in [
        (
            "node/node_positive.js",
            "ecmascript",
            "No standard-library declarations for imported module 'node:path'",
        ),
        (
            "deno/deno_positive.js",
            "node",
            "No static type information for 'Deno'",
        ),
        (
            "bun/bun_positive.js",
            "deno",
            "No static type information for 'Bun'",
        ),
        (
            "dom/dom_positive.js",
            "node",
            "No static type information for 'document'",
        ),
    ] {
        assert_fixture_rejected_with(fixture, environment_from_target(wrong_target), expected);
    }
}

#[test]
fn auto_target_detects_each_standard_library_environment() {
    for fixture in [
        "common/array_positive.js",
        "node/node_positive.js",
        "deno/deno_positive.js",
        "deno/compat_positive.js",
        "bun/bun_positive.js",
        "dom/dom_positive.js",
    ] {
        assert_fixture_valid(fixture, Environment::Auto);
    }
}

#[test]
fn built_positive_fixtures_run_on_installed_platform_runtimes() {
    let cases = [
        ("common/array_positive.js", Environment::Ecmascript, "node"),
        ("node/node_positive.js", Environment::Node, "node"),
        ("deno/deno_positive.js", Environment::Deno, "deno"),
        ("deno/compat_positive.js", Environment::Deno, "deno"),
        ("bun/bun_positive.js", Environment::Bun, "bun"),
    ];
    let mut executed_runners = BTreeSet::new();

    for (case_index, (fixture, environment, runner)) in cases.into_iter().enumerate() {
        let available = Command::new(runner)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !available {
            eprintln!("skipping {fixture}: {runner} is not installed");
            continue;
        }

        let output_path = std::env::temp_dir().join(format!(
            "refinejs-prelude-{runner}-{}-{case_index}.mjs",
            process::id()
        ));
        let path = fixture_path(fixture);
        let file_name = path.display().to_string();
        let (source, annotations) = parse_fixture(fixture);
        let errors =
            checker::check_source_with_environment(&source, &file_name, &annotations, environment);
        assert!(
            errors.is_empty(),
            "check failed for {fixture} under {environment}:\n{}",
            diagnostics(&errors)
        );
        let transformed = transpiler::transpile(&source, &file_name, &annotations)
            .unwrap_or_else(|error| panic!("transpile failed for {fixture}: {error}"));
        let code = format!("{}\n\n{transformed}", runtime::runtime_block());
        fs::write(&output_path, code)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", output_path.display()));

        let output = match runner {
            "node" => Command::new(runner).arg(&output_path).output(),
            "deno" => Command::new(runner)
                .args(["run", "--quiet"])
                .arg(&output_path)
                .output(),
            "bun" => Command::new(runner).arg("run").arg(&output_path).output(),
            _ => unreachable!("unsupported runtime test runner"),
        }
        .unwrap_or_else(|error| panic!("failed to run {runner} for {fixture}: {error}"));
        let _ = fs::remove_file(&output_path);

        assert!(
            output.status.success(),
            "{runner} failed for {fixture}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        executed_runners.insert(runner);
    }

    assert!(
        executed_runners.contains("node"),
        "Node is required by the existing integration-test contract"
    );
}
