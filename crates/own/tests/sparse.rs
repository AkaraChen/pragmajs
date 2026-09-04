//! Sparse `/*#own` payloads filled from a compiler-type test double.

use pragma_own::{
    check_parsed_with, check_parsed_with_payloads, omitted_payload_offsets, RuleKind, Runtime,
};
use pragma_parse::{parse, Allocator};
use std::collections::HashMap;

const SPARSE_UNIQUE: &str = r#"
/*#own type: (buf: unique) => void */
function process(buf: Buffer) {
}
"#;

#[test]
fn sparse_unique_param_forgets_when_payload_is_filled() {
    let filename = "sparse.ts";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, SPARSE_UNIQUE);
    assert!(
        parsed.diagnostics.is_empty(),
        "{:?}",
        parsed.diagnostics
    );
    let offsets = omitted_payload_offsets(filename, SPARSE_UNIQUE, &parsed.program);
    assert!(
        !offsets.is_empty(),
        "expected omitted payload offsets, got {offsets:?}"
    );
    let mut payloads = HashMap::new();
    for offset in offsets {
        payloads.insert(offset, "Buffer".to_string());
    }
    let result = check_parsed_with_payloads(
        filename,
        SPARSE_UNIQUE,
        &parsed.program,
        Runtime::None,
        Some(&payloads),
    );
    assert!(
        result.kinds().contains(&RuleKind::UniqueForget),
        "expected unique-forget after filling Buffer, got {:?}",
        result.formatted_lines()
    );
}

#[test]
fn sparse_unique_const_arrow_forgets_when_payload_is_filled() {
    let source = r#"
/*#own type: (buf: unique) => void */
const process = (buf: Buffer) => {};
"#;
    let filename = "sparse-arrow.ts";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, source);
    assert!(
        parsed.diagnostics.is_empty(),
        "{:?}",
        parsed.diagnostics
    );
    let offsets = omitted_payload_offsets(filename, source, &parsed.program);
    assert!(
        !offsets.is_empty(),
        "expected omitted payload offsets, got {offsets:?}"
    );
    let mut payloads = HashMap::new();
    for offset in offsets {
        payloads.insert(offset, "Buffer".to_string());
    }
    let result = check_parsed_with_payloads(
        filename,
        source,
        &parsed.program,
        Runtime::None,
        Some(&payloads),
    );
    assert!(
        !result.kinds().contains(&RuleKind::MissingType),
        "const-arrow should fill at param/return spans, got {:?}",
        result.formatted_lines()
    );
    assert!(
        result.kinds().contains(&RuleKind::UniqueForget),
        "expected unique-forget after filling Buffer on const-arrow, got {:?}",
        result.formatted_lines()
    );
}

#[test]
fn sparse_unique_without_payloads_reports_missing_type() {
    let filename = "sparse.ts";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, SPARSE_UNIQUE);
    let result = check_parsed_with(filename, SPARSE_UNIQUE, &parsed.program, Runtime::None);
    assert!(
        result.kinds().contains(&RuleKind::MissingType),
        "expected missing-type without compiler evidence, got {:?}",
        result.formatted_lines()
    );
    assert!(result.failed());
}
