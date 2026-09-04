//! Sparse `/*#rt` bases filled from a compiler-type test double.

use pragma_rt::{
    checker, parser,
    prelude::Environment,
    type_provider::{
        CompilerTypeAnalysis, CompilerTypeAtOffset, CompilerTypeProvider,
        CompilerTypeProviderError, CompilerTypeRequest,
    },
};
use std::path::Path;

struct NumberProvider;

impl CompilerTypeProvider for NumberProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        Ok(CompilerTypeAnalysis {
            types: request
                .byte_offsets
                .iter()
                .map(|byte_offset| CompilerTypeAtOffset {
                    byte_offset: *byte_offset,
                    utf16_offset: *byte_offset as u32,
                    rendered_type: Some("number".to_string()),
                    call_return_types: Vec::new(),
                    definition_paths: vec!["file:///typescript/lib/lib.es2025.d.ts".to_string()],
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

#[test]
fn parses_predicate_without_type_keyword() {
    let source = "/*#rt | x > 0 */\nconst x: number = 9;\n";
    let annotations = parsed_annotations(source, "no-type-keyword.ts");
    assert!(
        annotations
            .iter()
            .any(|a| matches!(a.ty.base, pragma_rt::syntax::BaseType::Omitted)
                && a.ty.predicate.is_some()),
        "expected omitted base with a predicate, got {annotations:#?}"
    );
}

fn parsed_annotations(source: &str, file_name: &str) -> Vec<pragma_rt::syntax::Annotation> {
    parser::parse_file(source, file_name)
        .expect("fixture must parse")
        .annotations
}

#[test]
fn sparse_predicate_fails_when_compiler_supplies_number() {
    let source = r#"
/*#rt type: | x > 0 */
const x: number = 0;
"#;
    let file_name = "sparse-fail.ts";
    let annotations = parsed_annotations(source, file_name);
    assert!(
        annotations
            .iter()
            .any(|a| matches!(a.ty.base, pragma_rt::syntax::BaseType::Omitted)),
        "expected an omitted base, got {annotations:#?}"
    );
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &NumberProvider,
        Path::new("/tmp/pragmajs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("does not satisfy its refinement")
            || error.message.contains("Initializer")),
        "expected a refinement finding after filling number, got {errors:#?}"
    );
}

#[test]
fn sparse_predicate_proves_when_compiler_supplies_number() {
    let source = r#"
/*#rt type: | x > 0 */
const x: number = 9;
"#;
    let file_name = "sparse-ok.ts";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &NumberProvider,
        Path::new("/tmp/pragmajs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "expected a clean proof after filling number, got {errors:#?}"
    );
}

struct FunctionContractProvider;

impl CompilerTypeProvider for FunctionContractProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        Ok(CompilerTypeAnalysis {
            types: request
                .byte_offsets
                .iter()
                .map(|byte_offset| {
                    let rest = request.source.get(*byte_offset..).unwrap_or("");
                    let rendered_type = if rest.starts_with("function") {
                        Some("(n: number) => number".to_string())
                    } else {
                        Some("number".to_string())
                    };
                    CompilerTypeAtOffset {
                        byte_offset: *byte_offset,
                        utf16_offset: *byte_offset as u32,
                        rendered_type,
                        call_return_types: Vec::new(),
                        definition_paths: vec![
                            "file:///typescript/lib/lib.es2025.d.ts".to_string(),
                        ],
                    }
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

#[test]
fn sparse_function_contract_fills_param_and_return_from_distinct_renderings() {
    let source = r#"
/*#rt type: (n: | n > 0) => | $ > 0 */
function incorrectlyPositive(n: number) {
  return 0;
}
"#;
    let file_name = "sparse-fn.ts";
    let annotations = parsed_annotations(source, file_name);
    assert!(
        annotations.iter().any(|annotation| {
            matches!(
                annotation.target,
                pragma_rt::syntax::AnnotationTarget::Param { .. }
            ) && matches!(annotation.ty.base, pragma_rt::syntax::BaseType::Omitted)
        }),
        "expected an omitted param base, got {annotations:#?}"
    );
    assert!(
        annotations.iter().any(|annotation| {
            matches!(
                annotation.target,
                pragma_rt::syntax::AnnotationTarget::Return { .. }
            ) && matches!(annotation.ty.base, pragma_rt::syntax::BaseType::Omitted)
        }),
        "expected an omitted return base, got {annotations:#?}"
    );
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &FunctionContractProvider,
        Path::new("/tmp/pragmajs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("does not satisfy its refinement")
            || error.message.contains("Return value")),
        "expected a refinement finding after filling number from a function rendering, got {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("omitted a base type")),
        "function-contract bases should fill, got {errors:#?}"
    );
}

#[test]
fn sparse_predicate_without_compiler_reports_missing_type() {
    let source = r#"
/*#rt type: | x > 0 */
const x: number = 9;
"#;
    let file_name = "sparse-missing.ts";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("omitted a base type")),
        "expected missing-type without compiler evidence, got {errors:#?}"
    );
}
