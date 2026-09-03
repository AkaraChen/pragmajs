use crate::syntax::*;
use std::path::Path;

pub fn check_source_with_environment(
    source: &str,
    file_name: &str,
    annotations: &[Annotation],
    environment: crate::prelude::Environment,
) -> Vec<RtError> {
    crate::verifier::verify_source(source, file_name, annotations, environment, None)
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
    let hints = crate::compiler_hints::analyze_source(provider, source, config_path, source_path)?;
    let mut errors = compiler_errors(source, file_name, source_path, hints.diagnostics());
    if errors.is_empty() {
        errors.extend(crate::verifier::verify_source(
            source,
            file_name,
            annotations,
            environment,
            Some(&hints),
        ));
    }
    Ok(errors)
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
                utf16_offset_to_line_column(source, diagnostic.range.start_utf16)
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

fn utf16_offset_to_line_column(source: &str, target: u32) -> (u32, u32) {
    let mut offset = 0u32;
    let mut line = 1u32;
    let mut column = 1u32;
    for character in source.chars() {
        if offset >= target {
            break;
        }
        offset = offset.saturating_add(character.len_utf16() as u32);
        if character == '\n' {
            line = line.saturating_add(1);
            column = 1;
        } else {
            column = column.saturating_add(character.len_utf16() as u32);
        }
    }
    (line, column)
}
