//! Filename + source → oxc `Program` (with comments) and semantic graph.

use oxc::parser::{ParseOptions, Parser};
use oxc::semantic::SemanticBuilder;
use oxc::span::{Span, SourceType};

pub use oxc::allocator::Allocator;
pub use oxc::ast::ast::Program;
pub use oxc::diagnostics::OxcDiagnostic as ParseDiagnostic;
pub use oxc::semantic::Semantic;

/// SourceType from a filename. Module mode is always on (both checkers treat
/// files as ESM). Unknown extensions fall back like own’s previous recipe.
pub fn source_type_for_path(filename: &str) -> SourceType {
    SourceType::from_path(filename)
        .unwrap_or_else(|_| {
            let lower = filename.to_ascii_lowercase();
            if lower.ends_with(".ts")
                || lower.ends_with(".tsx")
                || lower.ends_with(".mts")
                || lower.ends_with(".cts")
            {
                SourceType::ts()
            } else {
                SourceType::mjs()
            }
        })
        .with_module(true)
}

pub struct Parsed<'a> {
    pub program: Program<'a>,
    /// Structured Oxc diagnostics. Callers decide whether they fail the file.
    pub diagnostics: Vec<ParseDiagnostic>,
}

pub fn parse<'a>(allocator: &'a Allocator, filename: &str, source: &'a str) -> Parsed<'a> {
    let source_type = source_type_for_path(filename);
    let ret = Parser::new(allocator, source, source_type)
        .with_options(ParseOptions::default())
        .parse();
    Parsed {
        program: ret.program,
        diagnostics: ret.diagnostics.into_vec(),
    }
}

/// Select a diagnostic's primary source span, falling back to its first label.
///
/// Oxc offsets are zero-based UTF-8 byte offsets. A diagnostic without labels
/// has no source span and is returned as `None` rather than being assigned a
/// synthetic position.
pub fn diagnostic_span(diagnostic: &ParseDiagnostic) -> Option<Span> {
    diagnostic
        .labels
        .iter()
        .find(|label| label.primary())
        .or_else(|| diagnostic.labels.first())
        .map(|label| label.span())
}

pub fn semantic_graph<'a>(program: &'a Program<'a>) -> Semantic<'a> {
    SemanticBuilder::new().build(program).semantic
}

pub fn unresolved_root_names(semantic: &Semantic<'_>) -> Vec<String> {
    let mut names: Vec<String> = semantic
        .scoping()
        .root_unresolved_references()
        .keys()
        .map(|name| name.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}
