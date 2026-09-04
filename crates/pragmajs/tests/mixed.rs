//! Drives the shipped combined check: one parse, then own and rt on that program.

use pragma_own::RuleKind;
use pragma_parse::{parse, Allocator};
use pragmajs::{
    check_parsed, emit_source, CheckOptions, CheckerSelection, CombinedError, CompilerMode,
    CompilerOptions,
};
use std::fs;
use std::path::Path;

const MIXED_ERRORS: &str = include_str!("fixtures/mixed-errors.js");
const MIXED_OK: &str = include_str!("fixtures/mixed-ok.js");

#[test]
fn mixed_own_and_rt_findings_from_one_parse() {
    let filename = "mixed-errors.js";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, MIXED_ERRORS);
    assert!(
        parsed.diagnostics.is_empty(),
        "mixed fixture should parse: {:?}",
        parsed.diagnostics
    );

    let result = check_parsed(filename, MIXED_ERRORS, &parsed, &CheckOptions::default())
        .expect("combined check");

    assert!(
        result.own.kinds().contains(&RuleKind::UniqueForget),
        "expected unique-forget from own on the same parse, got {:?}",
        result.own.formatted_lines()
    );
    assert!(
        result
            .rt
            .iter()
            .any(|error| error.message.contains("does not satisfy its refinement")),
        "expected an rt refinement finding from the same parse, got {:#?}",
        result.rt
    );
    assert!(result.failed());
}

#[test]
fn mixed_ok_has_no_findings_from_one_parse() {
    let filename = "mixed-ok.js";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, MIXED_OK);
    assert!(
        parsed.diagnostics.is_empty(),
        "mixed ok fixture should parse: {:?}",
        parsed.diagnostics
    );

    let result = check_parsed(filename, MIXED_OK, &parsed, &CheckOptions::default())
        .expect("combined check");
    assert!(
        !result.failed(),
        "expected a clean mixed file, got:\n{}",
        result.formatted_lines().join("\n")
    );
}

#[test]
fn emit_preserves_runtime_asserts() {
    let result = emit_source("mixed-ok.js", MIXED_OK, &CheckOptions::default()).expect("emit");
    assert!(
        !result.check.failed(),
        "{:?}",
        result.check.formatted_lines()
    );
    let code = result.code.expect("emitted javascript");
    assert!(
        code.contains("__rt.assert"),
        "missing __rt.assert in:\n{code}"
    );
    assert!(code.contains("Math.sqrt"), "missing Math.sqrt in:\n{code}");
}

#[test]
fn own_only_emit_omits_refinement_runtime_and_asserts() {
    let result = emit_source(
        "mixed-ok.js",
        MIXED_OK,
        &CheckOptions {
            checker: CheckerSelection::Own,
            compiler: CompilerMode::Off,
            ..CheckOptions::default()
        },
    )
    .expect("own-only emit");
    assert!(
        !result.check.failed(),
        "{:?}",
        result.check.formatted_lines()
    );
    let code = result.code.expect("emitted javascript");
    assert!(code.contains("Math.sqrt"), "missing program code:\n{code}");
    assert!(
        !code.contains("__rt.assert"),
        "own-only output contains an rt assertion:\n{code}"
    );
    assert!(
        !code.contains("const __rt ="),
        "own-only output contains the rt runtime:\n{code}"
    );
}

#[test]
fn own_only_leaves_rt_results_empty() {
    let filename = "mixed-errors.js";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, MIXED_ERRORS);
    let options = CheckOptions {
        checker: CheckerSelection::Own,
        compiler: CompilerMode::Off,
        ..CheckOptions::default()
    };

    let result = check_parsed(filename, MIXED_ERRORS, &parsed, &options).expect("own-only check");
    assert!(result.own.kinds().contains(&RuleKind::UniqueForget));
    assert!(
        result.rt.is_empty(),
        "rt checker should not run: {:#?}",
        result.rt
    );
    assert!(
        result.rt_annotations.is_empty(),
        "rt annotation parser should not run"
    );
}

#[test]
fn rt_only_leaves_own_result_empty() {
    let filename = "mixed-errors.js";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, MIXED_ERRORS);
    let options = CheckOptions {
        checker: CheckerSelection::Rt,
        compiler: CompilerMode::Off,
        ..CheckOptions::default()
    };

    let result = check_parsed(filename, MIXED_ERRORS, &parsed, &options).expect("rt-only check");
    assert!(result.own.diagnostics.is_empty());
    assert!(result.own.sources.is_empty());
    assert!(!result.rt_annotations.is_empty());
    assert!(
        result
            .rt
            .iter()
            .any(|error| error.message.contains("does not satisfy its refinement")),
        "expected the rt finding: {:#?}",
        result.rt
    );
}

#[test]
fn own_only_does_not_parse_invalid_rt_annotation() {
    const INVALID_RT: &str = "/*#rt type: ( */\nconst value = 1;\n";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, "invalid-rt.js", INVALID_RT);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let own = check_parsed(
        "invalid-rt.js",
        INVALID_RT,
        &parsed,
        &CheckOptions {
            checker: CheckerSelection::Own,
            compiler: CompilerMode::Off,
            ..CheckOptions::default()
        },
    )
    .expect("own-only check");
    assert!(own.rt.is_empty());
    assert!(own.rt_annotations.is_empty());

    let rt = check_parsed(
        "invalid-rt.js",
        INVALID_RT,
        &parsed,
        &CheckOptions {
            checker: CheckerSelection::Rt,
            compiler: CompilerMode::Off,
            ..CheckOptions::default()
        },
    )
    .expect("rt-only check reports annotation errors in-band");
    assert!(!rt.rt.is_empty(), "invalid rt annotation should be parsed");
}

#[test]
fn own_only_without_sparse_payloads_skips_auto_compiler_for_real_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mixed-errors.js");
    let source = fs::read_to_string(&path).expect("real fixture");
    let filename = path.to_string_lossy();
    let allocator = Allocator::new();
    let parsed = parse(&allocator, &filename, &source);
    assert!(
        pragma_own::omitted_payload_offsets(&filename, &source, &parsed.program).is_empty(),
        "fixture must exercise the no-sparse-payload path"
    );

    let auto = check_parsed(
        &filename,
        &source,
        &parsed,
        &CheckOptions {
            checker: CheckerSelection::Own,
            compiler: CompilerMode::Auto,
            ..CheckOptions::default()
        },
    )
    .expect("Auto must not enter compiler discovery");
    assert!(auto.own.kinds().contains(&RuleKind::UniqueForget));

    let invalid_corsa = path.with_file_name("definitely-not-a-corsa-executable");
    let error = check_parsed(
        &filename,
        &source,
        &parsed,
        &CheckOptions {
            checker: CheckerSelection::Own,
            compiler: CompilerMode::On(CompilerOptions {
                corsa_path: Some(invalid_corsa.to_string_lossy().into_owned()),
                tsconfig_path: None,
            }),
            ..CheckOptions::default()
        },
    )
    .expect_err("an explicit compiler option must still force compiler discovery");
    assert!(matches!(error, CombinedError::Path(_)), "{error}");
}

#[test]
fn rt_only_keeps_the_compiler_hint_path_for_real_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mixed-errors.js");
    let source = fs::read_to_string(&path).expect("real fixture");
    let filename = path.to_string_lossy();
    let allocator = Allocator::new();
    let parsed = parse(&allocator, &filename, &source);
    let invalid_corsa = path.with_file_name("definitely-not-a-corsa-executable");

    let error = check_parsed(
        &filename,
        &source,
        &parsed,
        &CheckOptions {
            checker: CheckerSelection::Rt,
            compiler: CompilerMode::On(CompilerOptions {
                corsa_path: Some(invalid_corsa.to_string_lossy().into_owned()),
                tsconfig_path: None,
            }),
            ..CheckOptions::default()
        },
    )
    .expect_err("rt-only must still enter compiler discovery");
    assert!(matches!(error, CombinedError::Path(_)), "{error}");
}
