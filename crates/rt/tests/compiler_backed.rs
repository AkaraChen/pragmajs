use pragma_rt::{
    checker, parser,
    prelude::Environment,
    type_provider::{
        CompilerDiagnostic, CompilerDiagnosticKind, CompilerDiagnosticSeverity, CompilerRange,
        CompilerTypeAnalysis, CompilerTypeAtOffset, CompilerTypeProvider,
        CompilerTypeProviderError, CompilerTypeRequest, CorsaTypeProvider,
    },
};
use std::{env, fs, path::Path};

#[derive(Debug)]
struct FakeProvider {
    typed_offset: usize,
    diagnostics: Vec<CompilerDiagnostic>,
}

#[derive(Debug)]
struct PermissiveFallbackProvider;

#[derive(Debug)]
struct NominalCollisionProvider;

#[derive(Debug)]
struct ContextualCallProvider;

#[derive(Debug)]
struct StringProvider;

#[derive(Debug)]
struct ImplementationFallbackProvider;

#[derive(Debug)]
struct ContainerProvenanceProvider;

#[derive(Debug)]
struct NumberArrayProvider;

fn declaration_paths(request: &CompilerTypeRequest, byte_offset: usize) -> Vec<String> {
    request
        .definition_byte_offsets
        .contains(&byte_offset)
        .then(|| "file:///typescript/lib/lib.es2025.d.ts".to_string())
        .into_iter()
        .collect()
}

impl CompilerTypeProvider for FakeProvider {
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
                    rendered_type: (*byte_offset == self.typed_offset)
                        .then(|| "number".to_string()),
                    call_return_types: Vec::new(),
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: self.diagnostics.clone(),
        })
    }
}

impl CompilerTypeProvider for PermissiveFallbackProvider {
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
                    rendered_type: Some("unknown".to_string()),
                    call_return_types: vec!["number".to_string()],
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for NominalCollisionProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        let fake_binding = request
            .source
            .find("externalValue")
            .expect("external value exists");
        let size_member = request.source.rfind("size").expect("size member exists");
        Ok(CompilerTypeAnalysis {
            types: request
                .byte_offsets
                .iter()
                .map(|byte_offset| CompilerTypeAtOffset {
                    byte_offset: *byte_offset,
                    utf16_offset: *byte_offset as u32,
                    rendered_type: match *byte_offset {
                        offset if offset == fake_binding => Some("BunFile".to_string()),
                        offset if offset == size_member => Some("number".to_string()),
                        _ => None,
                    },
                    call_return_types: Vec::new(),
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for ContextualCallProvider {
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
                    call_return_types: if request.callable_byte_offsets.contains(byte_offset) {
                        vec!["any".to_string()]
                    } else {
                        Vec::new()
                    },
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for StringProvider {
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
                    rendered_type: Some("string".to_string()),
                    call_return_types: Vec::new(),
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for ImplementationFallbackProvider {
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
                    definition_paths: request
                        .definition_byte_offsets
                        .contains(byte_offset)
                        .then(|| "file:///tmp/dependency.js".to_string())
                        .into_iter()
                        .collect(),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for ContainerProvenanceProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        let local_object = request
            .source
            .find("/** @returns")
            .and_then(|comment| request.source[..comment].rfind('{'));
        Ok(CompilerTypeAnalysis {
            types: request
                .byte_offsets
                .iter()
                .map(|byte_offset| CompilerTypeAtOffset {
                    byte_offset: *byte_offset,
                    utf16_offset: *byte_offset as u32,
                    rendered_type: if Some(*byte_offset) == local_object {
                        Some("ExternalBox".to_string())
                    } else if request.source[*byte_offset..].starts_with("externalGroups") {
                        Some("ExternalBox[][]".to_string())
                    } else if request.source[*byte_offset..].starts_with("externalBoxes") {
                        Some("ExternalBox[]".to_string())
                    } else {
                        Some("unknown".to_string())
                    },
                    call_return_types: request
                        .callable_byte_offsets
                        .contains(byte_offset)
                        .then(|| {
                            if request.source[*byte_offset..].starts_with("externalRunAndReturn") {
                                "ExternalBox[]".to_string()
                            } else {
                                "number".to_string()
                            }
                        })
                        .into_iter()
                        .collect(),
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

impl CompilerTypeProvider for NumberArrayProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        let proxy_start = request.source.find("new Proxy");
        Ok(CompilerTypeAnalysis {
            types: request
                .byte_offsets
                .iter()
                .map(|byte_offset| CompilerTypeAtOffset {
                    byte_offset: *byte_offset,
                    utf16_offset: *byte_offset as u32,
                    rendered_type: (proxy_start
                        .is_some_and(|start| (start..=start + "new ".len()).contains(byte_offset))
                        || request.source[*byte_offset..].starts_with("numbers"))
                    .then(|| "number[]".to_string())
                    .or_else(|| Some("number".to_string())),
                    call_return_types: request
                        .callable_byte_offsets
                        .contains(byte_offset)
                        .then(|| "number".to_string())
                        .into_iter()
                        .collect(),
                    definition_paths: declaration_paths(request, *byte_offset),
                })
                .collect(),
            diagnostics: Vec::new(),
        })
    }
}

fn parsed_annotations(source: &str, file_name: &str) -> Vec<pragma_rt::syntax::Annotation> {
    parser::parse_file(source, file_name)
        .expect("fixture must parse")
        .annotations
}

fn source_line(source: &str, needle: &str) -> u32 {
    source
        .lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u32 + 1)
        .unwrap_or_else(|| panic!("missing {needle:?} in test source"))
}

#[test]
fn compiler_hint_supplies_an_unmodeled_dom_member_type() {
    let source = r#"
const canvas = document.createElement("canvas");
/*#rt type: number */
const width = canvas.width;
"#;
    let file_name = "/tmp/refinejs-compiler-hint.js";
    let annotations = parsed_annotations(source, file_name);
    let baseline = checker::check_source_with_environment(
        source,
        file_name,
        &annotations,
        Environment::Browser,
    );
    assert!(
        baseline
            .iter()
            .any(|error| error.message.contains("No static property 'width'"))
    );

    let provider = FakeProvider {
        typed_offset: source.rfind("width").expect("member exists"),
        diagnostics: Vec::new(),
    };
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Browser,
        &provider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
}

#[test]
fn compiler_hint_types_a_member_whose_meta_property_receiver_is_unmodeled() {
    let source = r#"
/*#rt type: string */
const path = import.meta.path;
"#;
    let file_name = "/tmp/refinejs-compiler-import-meta.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Bun,
        &StringProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "compiler evidence for import.meta.path was not used: {errors:?}"
    );
}

#[test]
fn compiler_named_types_cannot_inherit_curated_nominal_refinements() {
    let source = r#"
const fake = externalValue;

/*#rt type: number | observed >= 0 */
const observed = fake.size;
"#;
    let file_name = "/tmp/refinejs-compiler-nominal-collision.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Bun,
        &NominalCollisionProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Initializer for 'observed' does not satisfy its refinement")),
        "compiler-named BunFile incorrectly inherited Bun's size refinement: {errors:?}"
    );
}

#[test]
fn compiler_call_evidence_cannot_be_laundered_by_a_contextual_binding() {
    let call_source = r#"
/*#rt type: number */
const parsed = JSON.parse("{}");
"#;
    let call_file = "/tmp/refinejs-compiler-contextual-call.ts";
    let call_annotations = parsed_annotations(call_source, call_file);
    let call_errors = checker::check_source_with_environment_and_compiler(
        call_source,
        call_file,
        &call_annotations,
        Environment::Ecmascript,
        &ContextualCallProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(call_file),
    )
    .expect("fake provider cannot fail");
    assert!(
        call_errors
            .iter()
            .any(|error| error.message.contains("Base type mismatch")),
        "contextual binding hid the call's any return: {call_errors:?}"
    );
}

#[test]
fn compiler_errors_block_refinement_fallback() {
    let source = "const value = Object.keys(undefined);";
    let file_name = "/tmp/refinejs-compiler-error.js";
    let annotations = parsed_annotations(source, file_name);
    let provider = FakeProvider {
        typed_offset: usize::MAX,
        diagnostics: vec![CompilerDiagnostic {
            file: file_name.to_string(),
            kind: CompilerDiagnosticKind::Semantic,
            severity: CompilerDiagnosticSeverity::Error,
            code: Some("2345".to_string()),
            source: Some("typescript".to_string()),
            message: "Argument is not assignable".to_string(),
            range: CompilerRange {
                start_utf16: 26,
                end_utf16: 35,
            },
        }],
    };
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert_eq!(
        errors.len(),
        1,
        "compiler errors must stop refinement checking"
    );
    assert!(
        errors[0]
            .message
            .contains("TypeScript Semantic error TS2345")
    );
}

#[test]
fn compiler_fallback_still_checks_nested_refined_calls() {
    let source = r#"
/*#rt type: (value: number | value > 0) => number | $ > 0 */
function positive(value) {
  return value;
}

console.table(positive(-1));
"#;
    let file_name = "/tmp/refinejs-compiler-nested-call.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Argument 1 to 'positive' does not satisfy its refinement")),
        "nested refined call was skipped by compiler fallback: {errors:?}"
    );
}

#[test]
fn compiler_fallback_does_not_trust_an_unrefined_local_function_body() {
    let source = r#"
/** @returns {number} */
function lied() {
  return JSON.parse('"not a number"');
}

/*#rt type: number */
const accepted = lied();
"#;
    let file_name = "/tmp/refinejs-compiler-local-function.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("Local function 'lied' requires an explicit refinement contract")
    }));
}

#[test]
fn compiler_fallback_does_not_trust_a_local_method_behind_a_declaration_shape() {
    let source = r#"
/** @type {ExternalBox} */
const box = {
  /** @returns {number} */
  lied() {
    return JSON.parse('"not a number"');
  },
};

/*#rt type: number */
const accepted = box.lied();
"#;
    let file_name = "/tmp/refinejs-compiler-local-member.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("cannot validate a locally implemented object")
        }),
        "contextually typed local member reached compiler fallback: {errors:?}"
    );
}

#[test]
fn compiler_fallback_rejects_computed_local_method_calls() {
    let source = r#"
/** @type {ExternalBox} */
const box = {
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
};

/*#rt type: number */
const accepted = box["lied"]();
"#;
    let file_name = "/tmp/refinejs-compiler-computed-local-member.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("cannot validate a locally implemented object")
    }));
}

#[test]
fn contextual_callbacks_preserve_local_implementation_provenance() {
    let source = r#"
const boxes = [1].map(() => ({
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
}));

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-contextual-callback.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        }),
        "contextual callback return lost local provenance: {errors:?}"
    );
}

#[test]
fn named_contextual_callbacks_preserve_return_provenance() {
    let source = r#"
/*#rt type: () => ExternalBox */
function makeBox() {
  return {
    /** @returns {number} */
    lied() {
      return JSON.parse('\"not a number\"');
    },
  };
}

const boxes = [1].map(makeBox);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-named-contextual-callback.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Locally implemented values cannot flow")),
        "named callback return lost local provenance: {errors:?}"
    );
}

#[test]
fn mutating_array_writes_provenance_through_receiver_aliases() {
    let source = r#"
const boxes = externalBoxes;
const alias = boxes;
const localBox = {
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
};
alias.push(localBox);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-container-mutation.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("mutating an alias did not taint the receiver: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(13),
        "the mutation itself failed instead of tainting its receiver: {errors:?}"
    );
}

#[test]
fn immediate_callbacks_join_captured_container_provenance() {
    let source = r#"
const boxes = externalBoxes;
const localBox = {
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
};
[1].map(() => boxes.push(localBox));

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-callback-container-mutation.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("callback mutation did not taint its capture: {errors:?}"));
    assert_eq!(error.loc.as_ref().map(|location| location.line), Some(12));
}

#[test]
fn callback_mutation_updates_receiver_derived_result_provenance() {
    let source = r#"
let boxes = externalBoxes;
const localBox = {
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
};
const filtered = boxes.filter(() => boxes.push(localBox) > 0);

/*#rt type: number */
const accepted = externalUnwrap(filtered);
"#;
    let file_name = "/tmp/refinejs-compiler-callback-derived-container.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Locally implemented values cannot flow")),
        "receiver-derived result ignored callback mutation: {errors:?}"
    );
}

#[test]
fn contextual_identity_callbacks_keep_declaration_backed_values_usable() {
    let source = r#"
const boxes = externalBoxes;
const mapped = boxes.map(value => value);

/*#rt type: number */
const accepted = externalUnwrap(mapped);
"#;
    let file_name = "/tmp/refinejs-compiler-contextual-identity.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "identity callback over declaration-backed values was over-tainted: {errors:?}"
    );
}

#[test]
fn parenthesized_callbacks_keep_declaration_backed_values_usable() {
    let source = r#"
let boxes = externalBoxes;
const mapped = boxes.map((value => value));

/*#rt type: number */
const accepted = externalUnwrap(mapped);
"#;
    let file_name = "/tmp/refinejs-compiler-parenthesized-callback.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "parenthesized immediate callback tainted unrelated values: {errors:?}"
    );
}

#[test]
fn contextual_array_parameter_mutations_taint_the_original_receiver() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
boxes.map((_value, _index, array) => array.push(localBox));

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-callback-array-parameter.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("callback array parameter lost receiver provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn contextual_element_mutations_taint_nested_receiver_elements() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const groups = externalGroups;
groups.map(group => group.push(localBox));

/*#rt type: number */
const accepted = externalUnwrapGroups(groups);
"#;
    let file_name = "/tmp/refinejs-compiler-callback-element-parameter.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("callback element mutation lost provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn reduce_accumulator_mutations_taint_the_initial_value() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const initial = externalBoxes;
[1].reduce((acc) => (acc.push(localBox), acc), initial);

/*#rt type: number */
const accepted = externalUnwrap(initial);
"#;
    let file_name = "/tmp/refinejs-compiler-reduce-accumulator.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("reduce accumulator lost provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn array_containment_provenance_survives_unmodeled_havoc() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const groups = [boxes];
externalNoop();
boxes.push(localBox);

/*#rt type: number */
const accepted = externalUnwrapGroups(groups);
"#;
    let file_name = "/tmp/refinejs-compiler-monotone-containment.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("heap havoc erased containment provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn container_containment_does_not_taint_children_in_reverse() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const groups = [boxes];
groups.push([localBox]);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-directional-containment.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "container taint flowed backward into an unchanged child: {errors:?}"
    );
}

#[test]
fn push_containment_provenance_survives_later_taint() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const groups = [];
groups.push(boxes);
externalNoop();
boxes.push(localBox);

/*#rt type: number */
const accepted = externalUnwrapGroups(groups);
"#;
    let file_name = "/tmp/refinejs-compiler-push-containment.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("push containment lost later provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn callback_created_containment_edges_join_outer_state() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const groups = [];
[1].map(() => groups.push(boxes));
externalNoop();
boxes.push(localBox);

/*#rt type: number */
const accepted = externalUnwrapGroups(groups);
"#;
    let file_name = "/tmp/refinejs-compiler-callback-containment.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("callback containment was not joined: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn container_taint_does_not_infect_child_derived_results() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const groups = [boxes];
groups.push([localBox]);
const filtered = boxes.filter(() => true);

/*#rt type: number */
const accepted = externalUnwrap(filtered);
"#;
    let file_name = "/tmp/refinejs-compiler-child-derived-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "container taint infected a clean child-derived value: {errors:?}"
    );
}

#[test]
fn callback_mutation_taints_ephemeral_receiver_derived_results() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const filtered = [externalBoxes].filter(group => group.push(localBox) > 0);

/*#rt type: number */
const accepted = externalUnwrapGroups(filtered);
"#;
    let file_name = "/tmp/refinejs-compiler-ephemeral-receiver-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("ephemeral receiver mutation lost provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn immediate_callbacks_preserve_unrelated_reference_identity() {
    let source = r#"
let boxes = externalBoxes;
const alias = boxes;
[1].map(value => value + 1);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-immediate-unrelated-reference.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "unrelated callback tainted aliases: {errors:?}"
    );
}

#[test]
fn immediate_callback_havoc_does_not_break_existing_aliases() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
let boxes = externalBoxes;
const alias = boxes;
[1].map(value => value + 1);
alias.push(localBox);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-immediate-alias.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("callback havoc broke receiver aliases: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn declaration_backed_globals_keep_stable_reference_provenance() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
externalBoxes.push(localBox);

/*#rt type: number */
const accepted = externalUnwrap(externalBoxes);
"#;
    let file_name = "/tmp/refinejs-compiler-stable-global.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("repeated global read lost provenance: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn direct_compiler_reference_arguments_are_materialized_and_tainted() {
    let source = r#"
externalMutate(externalBoxes);

/*#rt type: number */
const accepted = externalUnwrap(externalBoxes);
"#;
    let file_name = "/tmp/refinejs-compiler-direct-escaped-global.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("direct escaped global stayed trusted: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn scalar_nested_calls_do_not_escape_their_callee_reference() {
    let source = r#"
externalConsume(externalFactory());

/*#rt type: number */
const accepted = externalFactory();
"#;
    let file_name = "/tmp/refinejs-compiler-scalar-nested-call.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "nested scalar call tainted its declaration-backed callee: {errors:?}"
    );
}

#[test]
fn compiler_expressions_cannot_launder_implementation_backed_scalars() {
    let source = r#"
/*#rt type: number */
const accepted = true ? lied : 0;
"#;
    let file_name = "/tmp/refinejs-compiler-implementation-scalar.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ImplementationFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Compiler-backed scalar evidence cannot execute or depend on a locally implemented value")),
        "implementation-backed scalar reached a refinement assertion: {errors:?}"
    );
}

#[test]
fn compiler_expressions_keep_verified_refined_calls_usable() {
    let source = r#"
/*#rt type: (value: number) => number */
function identity(value) {
  return value;
}

/*#rt type: number */
const accepted = true ? identity(1) : 0;
"#;
    let file_name = "/tmp/refinejs-compiler-verified-scalar.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ImplementationFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "verified refined call inside compiler expression was rejected: {errors:?}"
    );
}

#[test]
fn compiler_expression_validation_commits_nested_reference_effects() {
    let source = r#"
const localBox = {
  /** @returns {number} */
  lied() { return JSON.parse('\"not a number\"'); },
};
const boxes = externalBoxes;
const ignored = false ? {} : (boxes.push(localBox), {});

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-expression-effects.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("compiler expression discarded nested effects: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn user_contract_boundaries_taint_reference_arguments() {
    let source = r#"
/*#rt type: (boxes: Array<ExternalBox>) => void */
function mayMutate(boxes) {}

const boxes = externalBoxes;
mayMutate(boxes);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-user-contract-reference-effect.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("user contract argument stayed trusted: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn nested_user_contract_effects_are_joined_from_compiler_arguments() {
    let source = r#"
/*#rt type: () => number */
function mayMutateCapturedState() { return 0; }

const boxes = externalBoxes;
externalConsume(mayMutateCapturedState());

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-nested-contract-effect.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("nested contract effects were discarded: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn escaped_local_contract_callbacks_taint_captured_references_and_results() {
    let source = r#"
/*#rt type: () => void */
function poisonGlobal() {}

const boxes = externalBoxes;
const callback = poisonGlobal;
const returned = externalRunAndReturn(boxes, callback);

/*#rt type: number */
const original = externalUnwrap(boxes);
/*#rt type: number */
const alias = externalUnwrap(returned);
"#;
    let file_name = "/tmp/refinejs-compiler-local-callback-effects.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let local_lines = errors
        .iter()
        .filter(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .filter_map(|error| error.loc.as_ref().map(|location| location.line))
        .collect::<Vec<_>>();
    assert_eq!(
        local_lines,
        [
            source_line(source, "const original"),
            source_line(source, "const alias")
        ],
        "escaped callback effects did not taint captures and aliases: {errors:?}"
    );
}

#[test]
fn escaped_container_of_local_callbacks_taints_captures() {
    let source = r#"
/*#rt type: () => void */
function poisonGlobal() {}

const boxes = externalBoxes;
const callbacks = [poisonGlobal];
externalRun(callbacks);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-contained-callback-effects.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("contained callback effects were missed: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn opaque_callbacks_taint_receiver_derived_results() {
    let source = r#"
/*#rt type: (group: Array<ExternalBox>) => boolean */
function inspect(group) { return true; }

const groups = externalGroups;
const filtered = groups.filter(inspect);

/*#rt type: number */
const accepted = externalUnwrapGroups(filtered);
"#;
    let file_name = "/tmp/refinejs-compiler-opaque-callback-result.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("opaque callback result stayed trusted: {errors:?}"));
    assert_eq!(
        error.loc.as_ref().map(|location| location.line),
        Some(source_line(source, "const accepted"))
    );
}

#[test]
fn scalar_callback_implementations_do_not_taint_scalar_results() {
    let source = r#"
/*#rt type: (callback: (value: number) => number) => number */
function consumeMappedNumbers(callback) {
  const numbers = [1].map(callback);
  return externalConsume(numbers);
}
"#;
    let file_name = "/tmp/refinejs-compiler-scalar-callback-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "a callback implementation tainted scalar return values: {errors:?}"
    );
}

#[test]
fn reference_callback_returns_preserve_local_provenance() {
    let source = r#"
/*#rt type: (callback: (value: number) => ExternalBox) => number */
function consumeMappedBoxes(callback) {
  const boxes = [1].map(callback);
  return externalUnwrap(boxes);
}
"#;
    let file_name = "/tmp/refinejs-compiler-reference-callback-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Locally implemented values cannot flow")),
        "a callback's reference return lost local provenance: {errors:?}"
    );
}

#[test]
fn array_callback_returns_preserve_local_reference_identity() {
    let source = r#"
/*#rt type: (callback: (value: number) => Array<number>) => number */
function consumeMappedArrays(callback) {
  const arrays = [1].map(callback);
  return externalConsume(arrays);
}
"#;
    let file_name = "/tmp/refinejs-compiler-array-callback-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Locally implemented values cannot flow")),
        "a callback's array identity lost local provenance: {errors:?}"
    );
}

#[test]
fn unmodeled_havoc_never_erases_existing_local_provenance() {
    let source = r#"
let numbers = new Proxy([1], {});
externalNoop();

/*#rt type: number */
const accepted = externalConsume(numbers);
"#;
    let file_name = "/tmp/refinejs-compiler-monotone-provenance.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &NumberArrayProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Locally implemented values cannot flow")),
        "unmodeled havoc erased existing local provenance: {errors:?}"
    );
}

#[test]
fn refined_function_parameters_cannot_smuggle_local_member_implementations() {
    let source = r#"
/*#rt type: (box: ExternalBox) => number */
function invoke(box) {
  return box.lied();
}
"#;
    let file_name = "/tmp/refinejs-compiler-local-member-parameter.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("cannot validate a locally implemented object")
    }));
}

#[test]
fn compiler_owned_calls_forget_heap_and_mutable_scalar_facts() {
    let source = r#"
const values = [1];
values.splice(0, 1);

/*#rt type: number | staleLength === 1 */
const staleLength = values.length;

/*#rt type: number | counter === 1 */
let counter = 1;
console.table();

/*#rt type: number | staleCounter === 1 */
const staleCounter = counter;
"#;
    let file_name = "/tmp/refinejs-compiler-effects.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let messages = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("Initializer for 'staleLength' does not satisfy its refinement"),
        "heap facts survived an unknown compiler-owned call:\n{messages}"
    );
    assert!(
        messages.contains("Initializer for 'staleCounter' does not satisfy its refinement"),
        "mutable scalar facts survived an unknown compiler-owned call:\n{messages}"
    );
}

#[test]
fn compiler_calls_taint_escaped_reference_containers() {
    let source = r#"
const boxes = externalBoxes;
externalMutate(boxes);

/*#rt type: number */
const accepted = externalUnwrap(boxes);
"#;
    let file_name = "/tmp/refinejs-compiler-escaped-container.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContainerProvenanceProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .unwrap_or_else(|| panic!("escaped reference container stayed trusted: {errors:?}"));
    assert_eq!(error.loc.as_ref().map(|location| location.line), Some(6));
}

#[test]
fn compiler_effects_do_not_taint_scalar_or_scalar_array_arguments() {
    let source = r#"
let value = 1;
const numbers = [1];
externalNoop();
externalRead(numbers);

/*#rt type: number */
const accepted = externalConsume(value);
externalConsume(numbers);
"#;
    let file_name = "/tmp/refinejs-compiler-clean-effects.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.is_empty(),
        "scalar-only values were given implementation provenance: {errors:?}"
    );
}

#[test]
fn catalog_calls_degrade_to_arbitrary_effects_after_unknown_code() {
    let source = r#"
let counter = 0;
const values = [1];
externalPatchBuiltins();
counter = 1;
values.push(2);

/*#rt type: number | observed === 1 */
const observed = counter;
"#;
    let file_name = "/tmp/refinejs-compiler-patched-builtins.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Initializer for 'observed' does not satisfy its refinement")),
        "a catalog call after unknown code kept stale mutable facts: {errors:?}"
    );
}

#[test]
fn compiler_fallback_cannot_bypass_or_export_refined_function_preconditions() {
    let source = r#"
/*#rt type: (value: number | value > 0) => void */
function positiveOnly(value) {}

const alias = positiveOnly;
alias(-1);
externalConsume(positiveOnly);
externalConsume([positiveOnly]);
"#;
    let file_name = "/tmp/refinejs-compiler-function-escape.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let messages = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("Calling refined function value 'alias' through an alias"),
        "refined alias call reached compiler fallback:\n{messages}"
    );
    assert_eq!(
        messages
            .matches("cannot escape to compiler-owned code")
            .count(),
        2,
        "direct and nested refined function escapes were not both rejected:\n{messages}"
    );
}

#[test]
fn compiler_fallback_rejects_catalog_identity_callable_escape() {
    let source = r#"
/*#rt type: (values: Array<number>) => void */
function arrayOnly(values) {}

/*#rt type: (values: DenseArray<number>) => void */
function denseOnly(values) {}
const denseAlias = denseOnly;

/*#rt type: (callback: (values: ReadonlyArray<number>) => void) => void */
function readonlyConsumer(callback) {}

externalConsume(arrayOnly);
externalConsume(denseAlias);
externalConsume(readonlyConsumer);
"#;
    let file_name = "/tmp/refinejs-compiler-catalog-callable-escape.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    let messages = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for function in ["arrayOnly", "denseAlias", "readonlyConsumer"] {
        assert!(
            messages.contains(&format!("Refined function '{function}' cannot escape")),
            "catalog-identity callable '{function}' escaped compiler validation:\n{messages}"
        );
    }
    assert_eq!(
        messages
            .matches("cannot escape to compiler-owned code")
            .count(),
        3,
        "catalog-identity callable escapes were not rejected exactly once each:\n{messages}"
    );
}

#[test]
fn compiler_effects_reach_mutable_bindings_hidden_by_a_block_scope() {
    let source = r#"
/*#rt type: number | outer === 1 */
let outer = 1;
function mutateOuter() {
  outer = 0;
}
const outerValues = [1];
function mutateOuterValues() {
  outerValues.pop();
}

{
  const outer = 2;
  externalRun(mutateOuter);
  const outerValues = [2];
  externalRun(mutateOuterValues);
}

/*#rt type: number | staleOuter === 1 */
const staleOuter = outer;

/*#rt type: number | staleHiddenLength === 1 */
const staleHiddenLength = outerValues.length;
"#;
    let file_name = "/tmp/refinejs-compiler-hidden-binding.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &PermissiveFallbackProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Initializer for 'staleOuter' does not satisfy its refinement")),
        "hidden outer binding kept a stale refinement: {errors:?}"
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Initializer for 'staleHiddenLength' does not satisfy its refinement")),
        "hidden outer heap qualifier was restored after invalidation: {errors:?}"
    );
}

#[test]
fn compiler_effects_revoke_catalog_identity_from_reassignable_bindings() {
    let source = r#"
let values = [1];
function replaceValues() {
  values = { length: -1 };
}
externalRun(replaceValues);

/*#rt type: number | observedLength >= 0 */
const observedLength = values.length;
"#;
    let file_name = "/tmp/refinejs-compiler-reassigned-catalog-value.js";
    let annotations = parsed_annotations(source, file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        source,
        file_name,
        &annotations,
        Environment::Ecmascript,
        &ContextualCallProvider,
        Path::new("/tmp/refinejs-tsconfig.json"),
        Path::new(file_name),
    )
    .expect("fake provider cannot fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Initializer for 'observedLength' does not satisfy its refinement")),
        "a reassignable binding kept catalog identity after unknown code: {errors:?}"
    );
}

#[test]
fn real_corsa_checks_unmodeled_dom_members_when_configured() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/browser");
    let provider = CorsaTypeProvider::new(executable, &root);

    for (source_name, config_name, should_pass) in [
        ("member_refinement.js", "tsconfig.json", true),
        ("member_invalid.js", "tsconfig.invalid.json", false),
    ] {
        let source_path = root.join(source_name);
        let config_path = root.join(config_name);
        let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
        let file_name = source_path.display().to_string();
        let annotations = parsed_annotations(&source, &file_name);
        let errors = checker::check_source_with_environment_and_compiler(
            &source,
            &file_name,
            &annotations,
            Environment::Browser,
            &provider,
            &config_path,
            &source_path,
        )
        .expect("real Corsa analysis must complete");
        assert_eq!(
            errors.is_empty(),
            should_pass,
            "unexpected result for {source_name}: {errors:?}"
        );
        if !should_pass {
            assert!(errors.iter().any(|error| error.message.contains("TS2769")));
        }
    }
}

#[test]
fn real_corsa_rejects_suppression_in_a_transitive_project_source() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/project");
    let source_path = root.join("entry.js");
    let config_path = root.join("tsconfig.json");
    let provider = CorsaTypeProvider::new(executable, &root);
    let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
    let file_name = source_path.display().to_string();
    let annotations = parsed_annotations(&source, &file_name);
    let error = checker::check_source_with_environment_and_compiler(
        &source,
        &file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        &config_path,
        &source_path,
    )
    .expect_err("transitive diagnostic suppression must invalidate compiler evidence");
    assert!(matches!(
        error,
        CompilerTypeProviderError::DiagnosticSuppression { path, directive }
            if path.ends_with("dependency.js") && directive == "@ts-nocheck"
    ));
}

#[test]
fn real_corsa_cannot_launder_an_unrefined_local_function_return() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/local-function");
    let source_path = root.join("lied.js");
    let config_path = root.join("tsconfig.json");
    let provider = CorsaTypeProvider::new(executable, &root);
    let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
    let file_name = source_path.display().to_string();
    let annotations = parsed_annotations(&source, &file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        &source,
        &file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        &config_path,
        &source_path,
    )
    .expect("real Corsa analysis must complete");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("Local function 'lied' requires an explicit refinement contract")
    }));
}

#[test]
fn real_corsa_cannot_launder_a_contextually_typed_local_method_return() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/local-member");
    let source_path = root.join("entry.js");
    let config_path = root.join("tsconfig.json");
    let provider = CorsaTypeProvider::new(executable, &root);
    let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
    let file_name = source_path.display().to_string();
    let annotations = parsed_annotations(&source, &file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        &source,
        &file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        &config_path,
        &source_path,
    )
    .expect("real Corsa analysis must complete");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("cannot validate a locally implemented object")
    }));
}

#[test]
fn real_corsa_tracks_contextual_callback_return_provenance() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/callback-provenance");
    let source_path = root.join("entry.js");
    let config_path = root.join("tsconfig.json");
    let provider = CorsaTypeProvider::new(executable, &root);
    let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
    let file_name = source_path.display().to_string();
    let annotations = parsed_annotations(&source, &file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        &source,
        &file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        &config_path,
        &source_path,
    )
    .expect("real Corsa analysis must complete");
    let provenance_errors = errors
        .iter()
        .filter(|error| {
            error
                .message
                .contains("Locally implemented values cannot flow")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        provenance_errors.len(),
        2,
        "identity map should pass while local callback return and capture mutation fail: {errors:?}"
    );
    assert_eq!(
        provenance_errors
            .iter()
            .filter_map(|error| error.loc.as_ref().map(|location| location.line))
            .collect::<Vec<_>>(),
        [14, 26]
    );
}

#[test]
fn real_corsa_cannot_launder_an_imported_implementation_return() {
    let Some(executable) = env::var_os("REFINEJS_CORSA_TEST_BIN") else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/compiler/imported-function");
    let source_path = root.join("entry.js");
    let config_path = root.join("tsconfig.json");
    let provider = CorsaTypeProvider::new(executable, &root);
    let source = fs::read_to_string(&source_path).expect("compiler fixture must be readable");
    let file_name = source_path.display().to_string();
    let annotations = parsed_annotations(&source, &file_name);
    let errors = checker::check_source_with_environment_and_compiler(
        &source,
        &file_name,
        &annotations,
        Environment::Ecmascript,
        &provider,
        &config_path,
        &source_path,
    )
    .expect("real Corsa analysis must complete");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("cannot validate a locally implemented object")
    }));
}
