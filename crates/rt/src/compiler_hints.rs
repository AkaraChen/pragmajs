//! Query planning and type normalization for compiler-backed checking.
//!
//! Corsa answers questions at source positions, while the refinement verifier
//! identifies expressions by their full Oxc spans. This module owns the
//! deterministic mapping between those two coordinate systems.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use oxc_allocator::Allocator;
use oxc_ast::ast::{ChainElement, Expression, Program, VariableDeclarator};
use oxc_ast_visit::{
    Visit,
    walk::{walk_expression, walk_variable_declarator},
};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};

use crate::{
    syntax::BaseType,
    type_provider::{
        CompilerDiagnostic, CompilerTypeProvider, CompilerTypeProviderError, CompilerTypeRequest,
        definitions_are_declaration_backed,
    },
};

/// Compiler evidence available for one Oxc expression span.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompilerHint {
    pub rendered_type: Option<String>,
    pub call_return_types: Vec<String>,
    pub rendered_type_is_declaration_backed: bool,
    pub call_is_declaration_backed: bool,
}

/// Compiler evidence indexed by the UTF-8 `(span.start, span.end)` of Oxc
/// expressions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompilerHints {
    hints: BTreeMap<(u32, u32), CompilerHint>,
    diagnostics: Vec<CompilerDiagnostic>,
}

impl CompilerHints {
    /// Return compiler evidence for one exact expression span.
    pub fn get(&self, span: Span) -> Option<&CompilerHint> {
        self.hints.get(&(span.start, span.end))
    }

    /// Return all diagnostics produced during the same compiler analysis.
    pub fn diagnostics(&self) -> &[CompilerDiagnostic] {
        &self.diagnostics
    }
}

/// Analyze `source` with a compiler provider and index its answers by Oxc span.
///
/// `source` must be the exact contents of `file_path`; Corsa opens that file
/// from disk while the query plan is calculated from this string.
pub fn analyze_source(
    provider: &dyn CompilerTypeProvider,
    source: &str,
    config_path: &Path,
    file_path: &Path,
) -> Result<CompilerHints, CompilerTypeProviderError> {
    let plan = CompilerQueryPlan::from_source(source, file_path);
    let analysis = provider.analyze(&CompilerTypeRequest {
        config_path: config_path.to_path_buf(),
        file_path: file_path.to_path_buf(),
        source: source.to_string(),
        byte_offsets: plan.byte_offsets.clone(),
        callable_byte_offsets: plan.callable_byte_offsets.clone(),
        definition_byte_offsets: plan.definition_byte_offsets.clone(),
    })?;

    let types_by_offset = analysis
        .types
        .into_iter()
        .map(|compiler_type| (compiler_type.byte_offset, compiler_type))
        .collect::<BTreeMap<_, _>>();
    let hints = plan
        .query_by_span
        .into_iter()
        .filter_map(|(span, offsets)| {
            let rendered_type = offsets
                .result
                .and_then(|offset| types_by_offset.get(&offset))
                .and_then(|compiler_type| compiler_type.rendered_type.clone());
            let rendered_type_is_declaration_backed = offsets
                .result
                .and_then(|offset| types_by_offset.get(&offset))
                .is_some_and(|compiler_type| {
                    definitions_are_declaration_backed(&compiler_type.definition_paths)
                });
            let call_return_types = offsets
                .callable
                .and_then(|offset| types_by_offset.get(&offset))
                .map_or_else(Vec::new, |compiler_type| {
                    compiler_type.call_return_types.clone()
                });
            let call_is_declaration_backed = offsets
                .callable
                .and_then(|offset| types_by_offset.get(&offset))
                .is_some_and(|compiler_type| {
                    definitions_are_declaration_backed(&compiler_type.definition_paths)
                });
            let hint = normalize_hint(
                rendered_type,
                call_return_types,
                rendered_type_is_declaration_backed,
                call_is_declaration_backed,
            );
            (hint.rendered_type.is_some() || !hint.call_return_types.is_empty())
                .then_some((span, hint))
        })
        .collect();

    Ok(CompilerHints {
        hints,
        diagnostics: analysis.diagnostics,
    })
}

fn normalize_hint(
    rendered_type: Option<String>,
    mut call_return_types: Vec<String>,
    rendered_type_is_declaration_backed: bool,
    call_is_declaration_backed: bool,
) -> CompilerHint {
    call_return_types.sort();
    call_return_types.dedup();
    CompilerHint {
        rendered_type,
        call_return_types,
        rendered_type_is_declaration_backed,
        call_is_declaration_backed,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueryOffsets {
    result: Option<usize>,
    callable: Option<usize>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct CompilerQueryPlan {
    query_by_span: BTreeMap<(u32, u32), QueryOffsets>,
    byte_offsets: Vec<usize>,
    callable_byte_offsets: Vec<usize>,
    definition_byte_offsets: Vec<usize>,
}

impl CompilerQueryPlan {
    fn from_source(source: &str, file_path: &Path) -> Self {
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(file_path)
            .unwrap_or_else(|_| SourceType::default())
            .with_module(true);
        let parsed = Parser::new(&allocator, source, source_type)
            .with_options(ParseOptions::default())
            .parse();
        Self::from_program(&parsed.program)
    }

    fn from_program(program: &Program<'_>) -> Self {
        let mut collector = QueryPlanCollector::default();
        collector.visit_program(program);
        let callable_byte_offsets = collector
            .query_by_span
            .values()
            .filter_map(|offsets| offsets.callable)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let byte_offsets = collector
            .query_by_span
            .values()
            .flat_map(|offsets| [offsets.result, offsets.callable])
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let definition_byte_offsets = collector.definition_byte_offsets.into_iter().collect();
        Self {
            query_by_span: collector.query_by_span,
            byte_offsets,
            callable_byte_offsets,
            definition_byte_offsets,
        }
    }

    #[cfg(test)]
    fn result_offset_for(&self, span: Span) -> Option<usize> {
        self.query_by_span
            .get(&(span.start, span.end))
            .and_then(|offsets| offsets.result)
    }

    #[cfg(test)]
    fn callable_offset_for(&self, span: Span) -> Option<usize> {
        self.query_by_span
            .get(&(span.start, span.end))
            .and_then(|offsets| offsets.callable)
    }

    #[cfg(test)]
    fn callable_byte_offsets(&self) -> &[usize] {
        &self.callable_byte_offsets
    }
}

#[derive(Debug, Default)]
struct QueryPlanCollector {
    query_by_span: BTreeMap<(u32, u32), QueryOffsets>,
    definition_byte_offsets: BTreeSet<usize>,
}

impl QueryPlanCollector {
    fn record_result(&mut self, span: Span, byte_offset: u32) {
        self.query_by_span
            .entry((span.start, span.end))
            .or_default()
            .result
            .get_or_insert(byte_offset as usize);
    }

    fn record_callable(&mut self, span: Span, byte_offset: u32) {
        self.query_by_span
            .entry((span.start, span.end))
            .or_default()
            .callable
            .get_or_insert(byte_offset as usize);
    }

    fn record_definition(&mut self, byte_offset: u32) {
        self.definition_byte_offsets.insert(byte_offset as usize);
    }

    fn has_evidence(&self, span: Span) -> bool {
        self.query_by_span
            .get(&(span.start, span.end))
            .is_some_and(|offsets| offsets.result.is_some() || offsets.callable.is_some())
    }
}

impl<'a> Visit<'a> for QueryPlanCollector {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        // Query the initializer itself before considering its contextual binding.
        // An annotation on the binding can widen or otherwise replace the type at
        // that position, while identifier/member/call positions retain evidence
        // about the expression that actually executes.
        walk_variable_declarator(self, declarator);
        if let (Some(initializer), Some(binding)) =
            (&declarator.init, declarator.id.get_binding_identifier())
            && !self.has_evidence(initializer.span())
        {
            self.record_result(initializer.span(), binding.span.start);
        }
    }

    fn visit_expression(&mut self, expression: &Expression<'a>) {
        let span = expression.span();
        match expression {
            Expression::Identifier(identifier) => {
                self.record_result(span, identifier.span.start);
                self.record_definition(identifier.span.start);
            }
            Expression::StaticMemberExpression(member) => {
                self.record_result(span, member.property.span.start);
                self.record_definition(member.property.span.start);
            }
            Expression::ObjectExpression(object) => {
                self.record_result(span, object.span.start);
            }
            Expression::CallExpression(call) => {
                if let Some(query_offset) = callable_offset(&call.callee) {
                    self.record_callable(span, query_offset);
                    self.record_definition(query_offset);
                }
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    if let Some(query_offset) = callable_offset(&call.callee) {
                        self.record_callable(span, query_offset);
                        self.record_definition(query_offset);
                    }
                }
                ChainElement::StaticMemberExpression(member) => {
                    self.record_result(span, member.property.span.start);
                    self.record_definition(member.property.span.start);
                }
                ChainElement::PrivateFieldExpression(member) => {
                    self.record_result(span, member.field.span.start);
                    self.record_definition(member.field.span.start);
                }
                ChainElement::ComputedMemberExpression(_)
                | ChainElement::TSNonNullExpression(_) => {}
            },
            _ => {}
        }
        walk_expression(self, expression);
    }
}

fn callable_offset(expression: &Expression<'_>) -> Option<u32> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.span.start),
        Expression::StaticMemberExpression(member) => Some(member.property.span.start),
        Expression::ComputedMemberExpression(_) => None,
        Expression::PrivateFieldExpression(member) => Some(member.field.span.start),
        Expression::ParenthesizedExpression(parenthesized) => {
            callable_offset(&parenthesized.expression)
        }
        Expression::TSAsExpression(assertion) => callable_offset(&assertion.expression),
        Expression::TSSatisfiesExpression(assertion) => callable_offset(&assertion.expression),
        Expression::TSNonNullExpression(non_null) => callable_offset(&non_null.expression),
        Expression::TSInstantiationExpression(instantiation) => {
            callable_offset(&instantiation.expression)
        }
        _ => None,
    }
}

/// Convert a compiler-rendered TypeScript type into refinejs's base type model.
///
/// This parser intentionally recognizes only structural forms that can be
/// represented without guessing. Unsupported or malformed forms remain a
/// `Named` type containing the compiler's original rendering.
pub fn parse_typescript_type(rendered: &str) -> BaseType {
    let rendered = rendered.trim();
    parse_type(rendered).unwrap_or_else(|| BaseType::Named(rendered.to_owned()))
}

fn parse_type(rendered: &str) -> Option<BaseType> {
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return None;
    }

    let unwrapped = strip_outer_parentheses(rendered)?;
    if unwrapped != rendered {
        return parse_type(unwrapped);
    }

    if contains_top_level_complex_operator(rendered)? {
        return Some(BaseType::Named(rendered.to_owned()));
    }

    let union_members = split_top_level(rendered, '|')?;
    if union_members.len() > 1 {
        let members = union_members
            .into_iter()
            .map(parse_type)
            .collect::<Option<Vec<_>>>()?;
        let mut unique = Vec::new();
        for member in members {
            if !unique.contains(&member) {
                unique.push(member);
            }
        }
        return match unique.len() {
            1 => unique.pop(),
            _ => Some(BaseType::Union(unique)),
        };
    }

    if let Some(element) = rendered.strip_suffix("[]") {
        let element = element.trim();
        if element.is_empty() || !delimiters_balanced(element) {
            return None;
        }
        return Some(BaseType::Array(Box::new(parse_type(element)?)));
    }

    if let Some((name, arguments)) = split_generic(rendered)? {
        let arguments = arguments
            .into_iter()
            .map(parse_type)
            .collect::<Option<Vec<_>>>()?;
        if name == "Array" && arguments.len() == 1 {
            return Some(BaseType::Array(Box::new(arguments.into_iter().next()?)));
        }
        return Some(BaseType::Generic(name.to_owned(), arguments));
    }

    if is_primitive(rendered) {
        return Some(BaseType::Primitive(rendered.to_owned()));
    }
    if let Some(primitive) = literal_primitive(rendered) {
        return Some(BaseType::Primitive(primitive.to_owned()));
    }
    if is_named_type(rendered) {
        return Some(BaseType::Named(rendered.to_owned()));
    }
    Some(BaseType::Named(rendered.to_owned()))
}

fn is_primitive(rendered: &str) -> bool {
    matches!(
        rendered,
        "any"
            | "bigint"
            | "boolean"
            | "never"
            | "null"
            | "number"
            | "object"
            | "string"
            | "symbol"
            | "undefined"
            | "unknown"
            | "void"
    )
}

fn literal_primitive(rendered: &str) -> Option<&'static str> {
    if matches!(rendered, "true" | "false") {
        return Some("boolean");
    }
    if is_quoted(rendered, '\'') || is_quoted(rendered, '"') || is_quoted(rendered, '`') {
        return Some("string");
    }
    if rendered.strip_suffix('n').is_some_and(is_integer_literal) {
        return Some("bigint");
    }
    is_number_literal(rendered).then_some("number")
}

fn is_quoted(rendered: &str, quote: char) -> bool {
    rendered.starts_with(quote)
        && rendered.ends_with(quote)
        && rendered.len() >= quote.len_utf8() * 2
}

fn is_integer_literal(rendered: &str) -> bool {
    let rendered = rendered.strip_prefix('-').unwrap_or(rendered);
    if rendered.is_empty() {
        return false;
    }
    let without_separators = rendered.replace('_', "");
    if without_separators.is_empty() {
        return false;
    }
    if let Some(hex) = without_separators.strip_prefix("0x") {
        return !hex.is_empty() && hex.chars().all(|character| character.is_ascii_hexdigit());
    }
    if let Some(binary) = without_separators.strip_prefix("0b") {
        return !binary.is_empty()
            && binary
                .chars()
                .all(|character| matches!(character, '0' | '1'));
    }
    if let Some(octal) = without_separators.strip_prefix("0o") {
        return !octal.is_empty()
            && octal
                .chars()
                .all(|character| matches!(character, '0'..='7'));
    }
    without_separators
        .chars()
        .all(|character| character.is_ascii_digit())
}

fn is_number_literal(rendered: &str) -> bool {
    let without_separators = rendered.replace('_', "");
    if is_integer_literal(&without_separators) {
        return true;
    }
    without_separators.parse::<f64>().is_ok()
}

fn is_named_type(rendered: &str) -> bool {
    rendered.split('.').all(|part| {
        !part.is_empty()
            && part.chars().enumerate().all(|(index, character)| {
                character == '_'
                    || character == '$'
                    || character.is_alphanumeric()
                    || (index > 0 && matches!(character, '-' | '#'))
            })
    })
}

fn strip_outer_parentheses(mut rendered: &str) -> Option<&str> {
    loop {
        let Some(inner) = rendered
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
        else {
            return Some(rendered);
        };
        if matching_outer_parenthesis(rendered)? {
            rendered = inner.trim();
        } else {
            return Some(rendered);
        }
    }
}

fn matching_outer_parenthesis(rendered: &str) -> Option<bool> {
    let mut state = ScanState::default();
    for (index, character) in rendered.char_indices() {
        state.advance(character)?;
        if state.paren == 0 && index + character.len_utf8() < rendered.len() {
            return Some(false);
        }
    }
    Some(state.is_balanced())
}

fn split_generic(rendered: &str) -> Option<Option<(&str, Vec<&str>)>> {
    let mut state = ScanState::default();
    let mut open = None;
    let mut close = None;
    for (index, character) in rendered.char_indices() {
        if state.is_top_level() && character == '<' {
            open = Some(index);
        }
        state.advance(character)?;
        if open.is_some() && state.is_top_level() && character == '>' {
            close = Some(index);
            break;
        }
    }
    let Some(open) = open else {
        return Some(None);
    };
    let close = close?;
    if close + 1 != rendered.len() {
        return Some(None);
    }
    let name = rendered[..open].trim();
    if !is_named_type(name) {
        return Some(None);
    }
    let arguments = split_top_level(&rendered[open + 1..close], ',')?;
    if arguments.is_empty() {
        return Some(None);
    }
    Some(Some((name, arguments)))
}

fn split_top_level(rendered: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut state = ScanState::default();
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, character) in rendered.char_indices() {
        if state.is_top_level() && character == delimiter {
            let part = rendered[start..index].trim();
            if part.is_empty() {
                return None;
            }
            parts.push(part);
            start = index + character.len_utf8();
            continue;
        }
        state.advance(character)?;
    }
    if !state.is_balanced() {
        return None;
    }
    let part = rendered[start..].trim();
    if part.is_empty() {
        return None;
    }
    parts.push(part);
    Some(parts)
}

fn contains_top_level_complex_operator(rendered: &str) -> Option<bool> {
    let mut state = ScanState::default();
    let mut characters = rendered.char_indices().peekable();
    while let Some((_, character)) = characters.next() {
        if state.is_top_level()
            && (matches!(character, '&' | '?' | ':')
                || (character == '=' && characters.peek().is_some_and(|(_, next)| *next == '>')))
        {
            return Some(true);
        }
        state.advance(character)?;
    }
    state.is_balanced().then_some(false)
}

fn delimiters_balanced(rendered: &str) -> bool {
    let mut state = ScanState::default();
    rendered
        .chars()
        .all(|character| state.advance(character).is_some())
        && state.is_balanced()
}

#[derive(Debug, Default)]
struct ScanState {
    angle: usize,
    paren: usize,
    bracket: usize,
    brace: usize,
    quote: Option<char>,
    escaped: bool,
}

impl ScanState {
    fn is_top_level(&self) -> bool {
        self.angle == 0
            && self.paren == 0
            && self.bracket == 0
            && self.brace == 0
            && self.quote.is_none()
    }

    fn is_balanced(&self) -> bool {
        self.is_top_level() && !self.escaped
    }

    fn advance(&mut self, character: char) -> Option<()> {
        if let Some(quote) = self.quote {
            if self.escaped {
                self.escaped = false;
            } else if character == '\\' {
                self.escaped = true;
            } else if character == quote {
                self.quote = None;
            }
            return Some(());
        }

        match character {
            '\'' | '"' | '`' => self.quote = Some(character),
            '<' => self.angle += 1,
            '>' if self.angle > 0 => self.angle -= 1,
            '(' => self.paren += 1,
            ')' => self.paren = self.paren.checked_sub(1)?,
            '[' => self.bracket += 1,
            ']' => self.bracket = self.bracket.checked_sub(1)?,
            '{' => self.brace += 1,
            '}' => self.brace = self.brace.checked_sub(1)?,
            _ => {}
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CompilerQueryPlan, analyze_source, parse_typescript_type};
    use crate::syntax::BaseType;
    use crate::type_provider::{
        CompilerDiagnostic, CompilerDiagnosticKind, CompilerDiagnosticSeverity, CompilerRange,
        CompilerTypeAnalysis, CompilerTypeAtOffset, CompilerTypeProvider,
        CompilerTypeProviderError, CompilerTypeRequest,
    };
    use oxc_span::Span;
    use std::{cell::RefCell, collections::BTreeMap, path::Path};

    fn span_for(source: &str, text: &str) -> Span {
        let start = source.find(text).expect("test expression must exist");
        Span::new(start as u32, (start + text.len()) as u32)
    }

    #[derive(Default)]
    struct FakeProvider {
        requested_offsets: RefCell<Vec<usize>>,
        requested_callable_offsets: RefCell<Vec<usize>>,
        requested_definition_offsets: RefCell<Vec<usize>>,
        rendered_types: BTreeMap<usize, String>,
        call_return_types: BTreeMap<usize, Vec<String>>,
    }

    impl CompilerTypeProvider for FakeProvider {
        fn analyze(
            &self,
            request: &CompilerTypeRequest,
        ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
            *self.requested_offsets.borrow_mut() = request.byte_offsets.clone();
            *self.requested_callable_offsets.borrow_mut() = request.callable_byte_offsets.clone();
            *self.requested_definition_offsets.borrow_mut() =
                request.definition_byte_offsets.clone();
            let types = request
                .byte_offsets
                .iter()
                .copied()
                .map(|byte_offset| CompilerTypeAtOffset {
                    byte_offset,
                    utf16_offset: byte_offset as u32,
                    rendered_type: self.rendered_types.get(&byte_offset).cloned(),
                    call_return_types: if request.callable_byte_offsets.contains(&byte_offset) {
                        self.call_return_types
                            .get(&byte_offset)
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    definition_paths: request
                        .definition_byte_offsets
                        .contains(&byte_offset)
                        .then(|| "file:///typescript/lib/lib.es2025.d.ts".to_string())
                        .into_iter()
                        .collect(),
                })
                .collect();
            Ok(CompilerTypeAnalysis {
                types,
                diagnostics: vec![CompilerDiagnostic {
                    file: request.file_path.display().to_string(),
                    kind: CompilerDiagnosticKind::Semantic,
                    severity: CompilerDiagnosticSeverity::Error,
                    code: Some("2322".into()),
                    source: Some("typescript".into()),
                    message: "fixture diagnostic".into(),
                    range: CompilerRange {
                        start_utf16: 0,
                        end_utf16: 1,
                    },
                }],
            })
        }
    }

    #[test]
    fn plans_initializer_and_callable_queries_at_compiler_useful_offsets() {
        let source = "const mapped = [1].map(x => String(x));\n\
                      document.querySelector('canvas');\n\
                      mapped;";
        let plan = CompilerQueryPlan::from_source(source, Path::new("fixture.ts"));

        let mapped_initializer = span_for(source, "[1].map(x => String(x))");
        let map = source.find(".map").unwrap() + 1;
        let document_call = span_for(source, "document.querySelector('canvas')");
        let query_selector = source.find("querySelector").unwrap();
        let standalone_mapped = source.rfind("mapped").unwrap();
        let standalone_mapped_span = Span::new(
            standalone_mapped as u32,
            (standalone_mapped + "mapped".len()) as u32,
        );

        assert_eq!(plan.result_offset_for(mapped_initializer), None);
        assert_eq!(plan.callable_offset_for(mapped_initializer), Some(map));
        assert_eq!(
            plan.callable_offset_for(document_call),
            Some(query_selector)
        );
        assert_eq!(
            plan.result_offset_for(standalone_mapped_span),
            Some(standalone_mapped)
        );
        assert!(plan.byte_offsets.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn queries_object_literal_types_without_treating_the_literal_as_a_declaration() {
        let source = "const options = { fetch: () => 1 };";
        let plan = CompilerQueryPlan::from_source(source, Path::new("fixture.ts"));
        let object = span_for(source, "{ fetch: () => 1 }");
        let object_start = object.start as usize;

        assert_eq!(plan.result_offset_for(object), Some(object_start));
        assert!(!plan.definition_byte_offsets.contains(&object_start));
    }

    #[test]
    fn indexes_provider_results_and_preserves_diagnostics() {
        let source = "const mapped = [1].map(String);\n\
                      document.querySelector('canvas');";
        let mapped_initializer = span_for(source, "[1].map(String)");
        let document_call = span_for(source, "document.querySelector('canvas')");
        let query_selector = source.find("querySelector").unwrap();
        let map = source.find(".map").unwrap() + 1;
        let provider = FakeProvider {
            call_return_types: BTreeMap::from([
                (map, vec!["string[]".into()]),
                (
                    query_selector,
                    vec![
                        "Element | null".into(),
                        "Element | null".into(),
                        "E | null".into(),
                    ],
                ),
            ]),
            ..FakeProvider::default()
        };

        let hints = analyze_source(
            &provider,
            source,
            Path::new("/project/tsconfig.json"),
            Path::new("/project/fixture.ts"),
        )
        .unwrap();

        assert_eq!(
            hints
                .get(mapped_initializer)
                .map(|hint| hint.call_return_types.as_slice()),
            Some(["string[]".to_owned()].as_slice())
        );
        assert_eq!(
            hints
                .get(document_call)
                .map(|hint| hint.call_return_types.as_slice()),
            Some(["E | null".to_owned(), "Element | null".to_owned()].as_slice())
        );
        assert_eq!(hints.diagnostics().len(), 1);
        assert_eq!(hints.diagnostics()[0].message, "fixture diagnostic");

        let requested_offsets = provider.requested_offsets.borrow();
        assert!(
            requested_offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "provider offsets must be sorted and unique: {requested_offsets:?}"
        );
        assert_eq!(
            provider.requested_callable_offsets.borrow().as_slice(),
            [map, query_selector]
        );
    }

    #[test]
    fn keeps_nested_chain_queries_separate_and_skips_unsafe_computed_positions() {
        let source = "const final = factory().map(String).length;\n\
                      obj['value'];\n\
                      obj['f']();";
        let plan = CompilerQueryPlan::from_source(source, Path::new("fixture.js"));
        let initializer = span_for(source, "factory().map(String).length");
        let middle_call = span_for(source, "factory().map(String)");
        let inner_call = span_for(source, "factory()");
        let length = source.find("length").unwrap();
        let computed_member = span_for(source, "obj['value']");
        let computed_call = span_for(source, "obj['f']()");

        assert_eq!(plan.result_offset_for(initializer), Some(length));
        assert_eq!(
            plan.callable_offset_for(middle_call),
            Some(source.find("map").unwrap())
        );
        assert_eq!(
            plan.callable_offset_for(inner_call),
            Some(source.find("factory").unwrap())
        );
        assert_eq!(plan.result_offset_for(computed_member), None);
        assert_eq!(plan.callable_offset_for(computed_call), None);
        assert!(
            !plan
                .callable_byte_offsets()
                .contains(&source.find("'f'").unwrap())
        );
    }

    #[test]
    fn separates_a_call_result_from_the_callable_used_to_produce_it() {
        let source = "const fn: BoundCallable = makeFn();\nfn();";
        let plan = CompilerQueryPlan::from_source(source, Path::new("fixture.ts"));
        let initializer = span_for(source, "makeFn()");
        let invocation = span_for(source, "fn()");
        let binding_offset = source.find("fn").unwrap();
        let make_fn_offset = source.find("makeFn").unwrap();
        let invocation_offset = source.rfind("fn").unwrap();

        assert_eq!(plan.result_offset_for(initializer), None);
        assert_eq!(plan.callable_offset_for(initializer), Some(make_fn_offset));
        assert_eq!(
            plan.callable_offset_for(invocation),
            Some(invocation_offset)
        );
        assert_eq!(
            plan.callable_byte_offsets(),
            [make_fn_offset, invocation_offset]
        );
        assert!(!plan.callable_byte_offsets().contains(&binding_offset));
        assert!(!plan.byte_offsets.contains(&binding_offset));
    }

    #[test]
    fn keeps_a_callable_call_result_separate_from_both_invocations() {
        let source = "const fn: BoundCallable = makeFn();\nfn();";
        let binding_offset = source.find("fn").unwrap();
        let make_fn_offset = source.find("makeFn").unwrap();
        let invocation_offset = source.rfind("fn").unwrap();
        let initializer = span_for(source, "makeFn()");
        let invocation = span_for(source, "fn()");
        let provider = FakeProvider {
            rendered_types: BTreeMap::from([
                (make_fn_offset, "MakeFnCallable".into()),
                (invocation_offset, "BoundCallable".into()),
            ]),
            call_return_types: BTreeMap::from([
                (make_fn_offset, vec!["BoundCallable".into()]),
                (invocation_offset, vec!["number".into()]),
            ]),
            ..FakeProvider::default()
        };

        let hints = analyze_source(
            &provider,
            source,
            Path::new("/project/tsconfig.json"),
            Path::new("/project/fixture.ts"),
        )
        .unwrap();

        assert_eq!(
            hints.get(initializer),
            Some(&super::CompilerHint {
                rendered_type: None,
                call_return_types: vec!["BoundCallable".into()],
                rendered_type_is_declaration_backed: false,
                call_is_declaration_backed: true,
            })
        );
        assert_eq!(
            hints.get(invocation),
            Some(&super::CompilerHint {
                rendered_type: None,
                call_return_types: vec!["number".into()],
                rendered_type_is_declaration_backed: false,
                call_is_declaration_backed: true,
            })
        );
        assert_eq!(
            provider.requested_callable_offsets.borrow().as_slice(),
            [make_fn_offset, invocation_offset]
        );
        assert!(
            !provider
                .requested_offsets
                .borrow()
                .contains(&binding_offset)
        );
    }

    #[test]
    fn initializer_expression_evidence_wins_over_contextual_binding_annotations() {
        let source = "const fromIdentifier: unknown = sourceValue;\n\
                      const fromMember: unknown = sourceObject.value;\n\
                      const fromCall: unknown = makeValue();\n\
                      const fallback: number = left + right;";
        let plan = CompilerQueryPlan::from_source(source, Path::new("fixture.ts"));

        let identifier = span_for(source, "sourceValue");
        let member = span_for(source, "sourceObject.value");
        let call = span_for(source, "makeValue()");
        let fallback = span_for(source, "left + right");
        let property_offset = source.find(".value").unwrap() + 1;
        let callable_offset = source.find("makeValue").unwrap();
        let binding_offsets = [
            source.find("fromIdentifier").unwrap(),
            source.find("fromMember").unwrap(),
            source.find("fromCall").unwrap(),
        ];

        assert_eq!(
            plan.result_offset_for(identifier),
            Some(source.find("sourceValue").unwrap())
        );
        assert_eq!(plan.result_offset_for(member), Some(property_offset));
        assert_eq!(plan.result_offset_for(call), None);
        assert_eq!(plan.callable_offset_for(call), Some(callable_offset));
        assert_eq!(
            plan.result_offset_for(fallback),
            Some(source.find("fallback").unwrap())
        );
        for binding_offset in binding_offsets {
            assert!(
                !plan.byte_offsets.contains(&binding_offset),
                "contextual binding offset {binding_offset} replaced initializer evidence"
            );
        }
    }

    #[test]
    fn parses_nested_unions_arrays_and_generics() {
        assert_eq!(
            parse_typescript_type("Array<Promise<string | null>>"),
            BaseType::Array(Box::new(BaseType::Generic(
                "Promise".into(),
                vec![BaseType::Union(vec![
                    BaseType::Primitive("string".into()),
                    BaseType::Primitive("null".into()),
                ])],
            )))
        );
        assert_eq!(
            parse_typescript_type("(string | number)[][]"),
            BaseType::Array(Box::new(BaseType::Array(Box::new(BaseType::Union(vec![
                BaseType::Primitive("string".into()),
                BaseType::Primitive("number".into()),
            ],)))))
        );
        assert_eq!(
            parse_typescript_type("ReadonlyArray<Map<string, Array<number>>>"),
            BaseType::Generic(
                "ReadonlyArray".into(),
                vec![BaseType::Generic(
                    "Map".into(),
                    vec![
                        BaseType::Primitive("string".into()),
                        BaseType::Array(Box::new(BaseType::Primitive("number".into()))),
                    ],
                )],
            )
        );
    }

    #[test]
    fn parses_literals_and_preserves_complex_or_malformed_types() {
        assert_eq!(
            parse_typescript_type("'ready' | 42 | false"),
            BaseType::Union(vec![
                BaseType::Primitive("string".into()),
                BaseType::Primitive("number".into()),
                BaseType::Primitive("boolean".into()),
            ])
        );
        assert_eq!(
            parse_typescript_type("(value: string) => number | null"),
            BaseType::Named("(value: string) => number | null".into())
        );
        assert_eq!(
            parse_typescript_type("Array<string"),
            BaseType::Named("Array<string".into())
        );
        assert_eq!(
            parse_typescript_type("true | false"),
            BaseType::Primitive("boolean".into())
        );
    }
}
