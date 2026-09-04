use crate::syntax::*;
use oxc_ast::ast::Program;
use pragma_loc::utf16_offset_to_line_col;
use std::path::Path;

pub fn check_source_with_environment(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
) -> Vec<RtError> {
    check_source_with_environment_and_features(
        source,
        file_name,
        annotations,
        environment,
        crate::verifier::RtFeatures::default(),
    )
}

/// Experimental entry point for controlled verifier ablations.
pub fn check_source_with_environment_and_features(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    features: crate::verifier::RtFeatures,
) -> Vec<RtError> {
    let (filled, mut errors) = fill_omitted_bases(annotations, None);
    errors.extend(crate::verifier::verify_source_with_features(
        source,
        file_name,
        &filled,
        environment,
        None,
        features,
    ));
    errors
}

pub fn check_source_with_environment_and_compiler(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    provider: &dyn crate::type_provider::CompilerTypeProvider,
    config_path: &Path,
    source_path: &Path,
) -> Result<Vec<RtError>, crate::type_provider::CompilerTypeProviderError> {
    let extra = crate::parser::omitted_query_offsets(annotations);
    let hints = if extra.is_empty() {
        crate::compiler_hints::analyze_source(provider, source, config_path, source_path)?
    } else {
        let allocator = oxc_allocator::Allocator::default();
        let parsed = pragma_parse::parse(&allocator, file_name, source);
        crate::compiler_hints::analyze_program_with_offsets(
            provider,
            source,
            &parsed.program,
            config_path,
            source_path,
            &extra,
        )?
    };
    Ok(check_with_hints(
        source,
        file_name,
        annotations,
        environment,
        source_path,
        &hints,
    ))
}

/// Check a program already produced by `pragma_parse`.
pub fn check_program_with_environment(
    source: &str,
    file_name: &str,
    program: &Program<'_>,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
) -> Vec<RtError> {
    check_program_with_environment_and_features(
        source,
        file_name,
        program,
        annotations,
        environment,
        crate::verifier::RtFeatures::default(),
    )
}

/// Experimental parsed-program entry point for controlled verifier ablations.
pub fn check_program_with_environment_and_features(
    source: &str,
    file_name: &str,
    program: &Program<'_>,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    features: crate::verifier::RtFeatures,
) -> Vec<RtError> {
    let (filled, mut errors) = fill_omitted_bases(annotations, None);
    errors.extend(crate::verifier::verify_program_with_features(
        source,
        file_name,
        program,
        &filled,
        environment,
        None,
        features,
    ));
    errors
}

/// Like [`check_program_with_environment`], with compiler-backed hints from the
/// same parsed program (no second oxc parse).
pub fn check_program_with_environment_and_compiler(
    source: &str,
    file_name: &str,
    program: &Program<'_>,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    provider: &dyn crate::type_provider::CompilerTypeProvider,
    config_path: &Path,
    source_path: &Path,
) -> Result<Vec<RtError>, crate::type_provider::CompilerTypeProviderError> {
    let extra = crate::parser::omitted_query_offsets(annotations);
    let hints = crate::compiler_hints::analyze_program_with_offsets(
        provider,
        source,
        program,
        config_path,
        source_path,
        &extra,
    )?;
    Ok(check_program_with_hints(
        source,
        file_name,
        program,
        annotations,
        environment,
        source_path,
        &hints,
    ))
}

/// Verify using compiler hints already produced for this program.
pub fn check_program_with_hints(
    source: &str,
    file_name: &str,
    program: &Program<'_>,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    source_path: &Path,
    hints: &crate::compiler_hints::CompilerHints,
) -> Vec<RtError> {
    check_with_program_and_hints(
        source,
        file_name,
        Some(program),
        annotations,
        environment,
        source_path,
        hints,
    )
}

fn check_with_hints(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    source_path: &Path,
    hints: &crate::compiler_hints::CompilerHints,
) -> Vec<RtError> {
    check_with_program_and_hints(
        source,
        file_name,
        None,
        annotations,
        environment,
        source_path,
        hints,
    )
}

fn check_with_program_and_hints(
    source: &str,
    file_name: &str,
    program: Option<&Program<'_>>,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
    source_path: &Path,
    hints: &crate::compiler_hints::CompilerHints,
) -> Vec<RtError> {
    let mut errors = compiler_errors(source, file_name, source_path, hints.diagnostics());
    if !errors.is_empty() {
        return errors;
    }
    let (filled, mut fill_errors) = fill_omitted_bases(annotations, Some(hints));
    if program.is_none() {
        errors.extend(fill_errors);
        errors.extend(crate::verifier::verify_source(
            source,
            file_name,
            &filled,
            environment,
            Some(hints),
        ));
        return errors;
    }
    errors.append(&mut fill_errors);
    errors.extend(crate::verifier::verify_program(
        source,
        file_name,
        program.expect("program"),
        &filled,
        environment,
        Some(hints),
    ));
    errors
}

fn fill_omitted_bases(
    annotations: &[Annotation],
    hints: Option<&crate::compiler_hints::CompilerHints>,
) -> (Vec<Annotation>, Vec<RtError>) {
    let mut filled = Vec::with_capacity(annotations.len());
    let mut errors = Vec::new();
    for annotation in annotations {
        let mut annotation = annotation.clone();
        let label = annotation_label(&annotation.target);
        fill_refinement(
            &mut annotation.ty,
            annotation.query_offset,
            hints,
            &annotation.loc,
            &annotation.target,
            &label,
            &mut errors,
        );
        filled.push(annotation);
    }
    (filled, errors)
}

fn annotation_label(target: &AnnotationTarget) -> String {
    match target {
        AnnotationTarget::Param { param_name, .. } => param_name.clone(),
        AnnotationTarget::Return { function_name, .. } => format!("return of {function_name}"),
        AnnotationTarget::Variable { name, .. } => name.clone(),
    }
}

fn fill_refinement(
    ty: &mut RefinementType,
    query_offset: u32,
    hints: Option<&crate::compiler_hints::CompilerHints>,
    loc: &SourceLocation,
    target: &AnnotationTarget,
    label: &str,
    errors: &mut Vec<RtError>,
) {
    if matches!(ty.base, BaseType::Omitted) {
        match hints.and_then(|hints| hints.rendered_at(query_offset as usize)) {
            Some(rendered) => {
                ty.base = base_from_compiler_rendering(rendered, target);
            }
            None => {
                errors.push(RtError {
                    message: format!(
                        "/*#rt omitted a base type for `{label}` and TypeScript did not supply one"
                    ),
                    loc: Some(loc.clone()),
                });
            }
        }
    }
    if let BaseType::Function(params, ret) = &mut ty.base {
        for param in params {
            fill_refinement(
                &mut param.ty,
                query_offset,
                hints,
                loc,
                target,
                &param.name,
                errors,
            );
        }
        fill_refinement(ret, query_offset, hints, loc, target, label, errors);
    }
}

fn base_from_compiler_rendering(rendered: &str, target: &AnnotationTarget) -> BaseType {
    if let Some((params, ret)) = crate::compiler_hints::call_signature_parts(rendered) {
        return match target {
            AnnotationTarget::Return { .. } => {
                crate::compiler_hints::parse_typescript_type(&ret)
            }
            AnnotationTarget::Param { index, .. } => params
                .get(*index)
                .map(|ty| crate::compiler_hints::parse_typescript_type(ty))
                .unwrap_or_else(|| crate::compiler_hints::parse_typescript_type(rendered)),
            AnnotationTarget::Variable { .. } => {
                crate::compiler_hints::parse_typescript_type(rendered)
            }
        };
    }
    crate::compiler_hints::parse_typescript_type(rendered)
}

fn compiler_errors(
    source: &str,
    file_name: &str,
    source_path: &Path,
    diagnostics: &[crate::type_provider::CompilerDiagnostic],
) -> Vec<RtError> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity == crate::type_provider::CompilerDiagnosticSeverity::Error
        })
        .map(|diagnostic| {
            let diagnostic_is_for_source =
                diagnostic.file.is_empty() || Path::new(&diagnostic.file) == source_path;
            let (line, column) = if diagnostic_is_for_source {
                utf16_offset_to_line_col(source, diagnostic.range.start_utf16)
            } else {
                (1, 1)
            };
            let code = diagnostic
                .code
                .as_deref()
                .map_or(String::new(), |code| format!(" TS{code}"));
            RtError {
                message: format!(
                    "TypeScript {:?} error{code}: {}",
                    diagnostic.kind, diagnostic.message
                ),
                loc: Some(SourceLocation {
                    file: Some(if diagnostic.file.is_empty() {
                        file_name.to_string()
                    } else {
                        diagnostic.file.clone()
                    }),
                    line,
                    column,
                }),
            }
        })
        .collect()
}
