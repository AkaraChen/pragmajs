//! Drives the shipped combined check: one parse, then own and rt on that program.

use pragma_own::RuleKind;
use pragma_parse::{parse, Allocator};
use pragmajs::{check_parsed, emit_source, CheckOptions};

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
