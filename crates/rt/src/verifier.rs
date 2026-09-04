use crate::compiler_hints::{CompilerHints, parse_typescript_type};
use crate::prelude::{
    CallbackTiming, Environment, FunctionSignature, LibraryExport, LibraryRegistry, ReceiverEffect,
    SemanticRefinement,
};
use crate::syntax::{
    Annotation, AnnotationTarget, BaseType, BinaryOp, Literal, LogicalOp, PredicateExpr,
    RefinementType, RtError, SourceLocation,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Expression, Function, ObjectPropertyKind, Program, PropertyKind,
    SimpleAssignmentTarget, Statement,
};
use oxc_ast_visit::{Visit, walk::walk_expression};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{
    AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator, UpdateOperator,
};
use pragma_parse::parse;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use z3::{
    Fixedpoint, FuncDecl, SatResult, Solver, Sort as Z3Sort, Symbol,
    ast::{self, Ast, Bool, Dynamic, Float, Int as Z3Int, RoundingMode, String as Z3String},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Sort {
    Number,
    Int,
    Bool,
    String,
    Ref,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Term {
    Number(i64),
    Int(i64),
    Bool(bool),
    String(String),
    Var(String, Sort),
    Member(Box<Term>, String, Sort),
    Index(Box<Term>, Box<Term>, Sort),
    Pred(String, Box<Term>),
    ToNumber(Box<Term>),
    Add(Box<Term>, Box<Term>),
    Sub(Box<Term>, Box<Term>),
    Mul(Box<Term>, Box<Term>),
    Same(Box<Term>, Box<Term>),
    Eq(Box<Term>, Box<Term>),
    Ne(Box<Term>, Box<Term>),
    Gt(Box<Term>, Box<Term>),
    Lt(Box<Term>, Box<Term>),
    Ge(Box<Term>, Box<Term>),
    Le(Box<Term>, Box<Term>),
    And(Box<Term>, Box<Term>),
    Or(Box<Term>, Box<Term>),
    Not(Box<Term>),
}

impl Term {
    fn sort(&self) -> Sort {
        match self {
            Self::Number(_) | Self::ToNumber(_) => Sort::Number,
            Self::Int(_) => Sort::Int,
            Self::Add(left, right) | Self::Sub(left, right) | Self::Mul(left, right) => {
                if left.sort() == Sort::Int && right.sort() == Sort::Int {
                    Sort::Int
                } else {
                    Sort::Number
                }
            }
            Self::String(_) => Sort::String,
            Self::Bool(_)
            | Self::Eq(..)
            | Self::Same(..)
            | Self::Ne(..)
            | Self::Gt(..)
            | Self::Lt(..)
            | Self::Ge(..)
            | Self::Le(..)
            | Self::And(..)
            | Self::Or(..)
            | Self::Not(..) => Sort::Bool,
            Self::Pred(..) => Sort::Bool,
            Self::Var(_, sort) => *sort,
            Self::Member(_, _, sort) => *sort,
            Self::Index(_, _, sort) => *sort,
        }
    }
}

#[derive(Debug, Clone)]
struct Qualifier {
    value: Term,
    formula: Term,
}

#[derive(Debug, Clone)]
struct Value {
    term: Term,
    base: BaseType,
    declared_base: Option<BaseType>,
    qualifier: Option<Qualifier>,
    mutable: bool,
    /// Whether this value's runtime identity was established by the refinement
    /// checker or its selected prelude. Compiler-rendered named types are not
    /// enough: a project may declare its own `Element`, `BunFile`, and so on.
    catalog_trusted: bool,
    /// Whether this value contains a callable, getter, or class implementation
    /// authored in the checked source. A declaration file may describe its
    /// shape, but cannot validate that local implementation's runtime result.
    local_implementation: bool,
}

#[derive(Debug, Clone)]
struct Contract {
    params: Vec<(String, RefinementType)>,
    ret: RefinementType,
    predicate_params: Vec<String>,
    loc: SourceLocation,
}

#[derive(Debug, Clone)]
struct State {
    env: HashMap<String, Value>,
    /// Declaration-backed identifiers which are not lexical bindings in the
    /// checked file. Keep one value per binding so repeated reads preserve
    /// reference identity and provenance.
    compiler_bindings: HashMap<String, Value>,
    entry_params: HashMap<String, Term>,
    assumptions: Vec<Term>,
    /// Monotone reference relationships used only for implementation
    /// provenance. Unlike logical heap facts, these survive effect havoc.
    provenance_edges: Vec<ReferenceProvenanceEdge>,
    /// Reference terms already known to carry or contain a local
    /// implementation, including ephemeral values without a lexical binding.
    local_reference_provenance: HashSet<Term>,
    scopes: Vec<HashMap<String, (Option<Value>, bool)>>,
    uninitialized: std::collections::HashSet<String>,
    /// Unknown JavaScript can replace built-in prototypes or host methods.
    /// Once crossed, catalog refinements are no longer sound in this path.
    library_semantics_intact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReferenceProvenanceEdge {
    /// Both terms denote the same reference identity.
    Alias(Term, Term),
    /// `value` may be stored inside `container`. Taint flows from the value to
    /// the container, never in the opposite direction.
    ContainedBy { value: Term, container: Term },
}

impl Default for State {
    fn default() -> Self {
        Self {
            env: HashMap::new(),
            compiler_bindings: HashMap::new(),
            entry_params: HashMap::new(),
            assumptions: Vec::new(),
            provenance_edges: Vec::new(),
            local_reference_provenance: HashSet::new(),
            scopes: Vec::new(),
            uninitialized: HashSet::new(),
            library_semantics_intact: true,
        }
    }
}

/// A liquid subtyping obligation in Horn-clause form:
/// `assumptions => consequent`. Abstract predicate applications are reduced to
/// freely chosen Boolean atoms with congruence constraints before the Horn
/// rule is sent to Z3.
struct FixpointConstraint<'a> {
    assumptions: &'a [Term],
    consequent: &'a Term,
}

pub(crate) fn verify_source<'a>(
    source: &'a str,
    file_name: &'a str,
    annotations: &[Annotation],
    environment: Environment,
    compiler_hints: Option<&'a CompilerHints>,
) -> Vec<RtError> {
    let allocator = Allocator::default();
    let parsed = parse(&allocator, file_name, source);
    if !parsed.diagnostics.is_empty() {
        return vec![RtError {
            message: format!("JavaScript parse errors: {:?}", parsed.diagnostics),
            loc: None,
        }];
    }
    verify_program(
        source,
        file_name,
        &parsed.program,
        annotations,
        environment,
        compiler_hints,
    )
}

pub(crate) fn verify_program<'a>(
    source: &'a str,
    file_name: &'a str,
    program: &Program<'_>,
    annotations: &[Annotation],
    environment: Environment,
    compiler_hints: Option<&'a CompilerHints>,
) -> Vec<RtError> {
    let library = match crate::prelude::registry_for_program(environment, program) {
        Ok(library) => library,
        Err(error) => {
            return vec![RtError {
                message: error.to_string(),
                loc: None,
            }];
        }
    };
    let contracts = collect_contracts(annotations);
    let annotation_errors = validate_annotation_structure(annotations, &contracts);
    let variable_types = collect_variable_types(annotations);
    let mut verifier = Verifier {
        source,
        file_name,
        contracts,
        signatures: HashMap::new(),
        library,
        imports: HashMap::new(),
        top_level_bindings: HashSet::new(),
        has_unmodeled_import_effects: false,
        compiler_hints,
        variable_types,
        consumed_variable_types: HashSet::new(),
        errors: annotation_errors,
        fresh: 0,
    };
    verifier.verify_program(program);
    verifier.errors
}

#[derive(Debug, Clone)]
enum ImportBinding {
    Namespace { module: String },
    Export { module: String, export: String },
}

type ParameterAnnotations = HashMap<(String, u32), Vec<(usize, String, RefinementType)>>;

fn collect_contracts(annotations: &[Annotation]) -> HashMap<(String, u32), Contract> {
    let mut params = ParameterAnnotations::new();
    let mut returns: HashMap<(String, u32), (RefinementType, Vec<String>, SourceLocation)> =
        HashMap::new();
    for annotation in annotations {
        match &annotation.target {
            AnnotationTarget::Param {
                function_name,
                function_start,
                param_name,
                index,
            } => {
                params
                    .entry((function_name.clone(), *function_start))
                    .or_default()
                    .push((*index, param_name.clone(), annotation.ty.clone()));
            }
            AnnotationTarget::Return {
                function_name,
                function_start,
            } => {
                returns.insert(
                    (function_name.clone(), *function_start),
                    (
                        annotation.ty.clone(),
                        annotation.predicate_params.clone(),
                        annotation.loc.clone(),
                    ),
                );
            }
            AnnotationTarget::Variable { .. } => {}
        }
    }

    returns
        .into_iter()
        .map(
            |((name, declaration_start), (ret, predicate_params, loc))| {
                let key = (name.clone(), declaration_start);
                let mut function_params = params.remove(&key).unwrap_or_default();
                function_params.sort_by_key(|(index, _, _)| *index);
                let params = function_params
                    .into_iter()
                    .map(|(_, name, ty)| (name, ty))
                    .collect();
                (
                    key,
                    Contract {
                        params,
                        ret,
                        predicate_params,
                        loc,
                    },
                )
            },
        )
        .collect()
}

fn collect_variable_types(
    annotations: &[Annotation],
) -> HashMap<u32, (String, RefinementType, SourceLocation)> {
    annotations
        .iter()
        .filter_map(|annotation| match &annotation.target {
            AnnotationTarget::Variable {
                name,
                declaration_start,
            } => Some((
                *declaration_start,
                (name.clone(), annotation.ty.clone(), annotation.loc.clone()),
            )),
            _ => None,
        })
        .collect()
}

fn validate_annotation_structure(
    annotations: &[Annotation],
    contracts: &HashMap<(String, u32), Contract>,
) -> Vec<RtError> {
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for annotation in annotations {
        let key = match &annotation.target {
            AnnotationTarget::Param {
                function_name,
                function_start,
                index,
                ..
            } => format!("param:{function_name}:{function_start}:{index}"),
            AnnotationTarget::Return {
                function_name,
                function_start,
            } => format!("return:{function_name}:{function_start}"),
            AnnotationTarget::Variable {
                declaration_start, ..
            } => format!("variable:{declaration_start}"),
        };
        if !seen.insert(key) {
            errors.push(RtError {
                message: "Duplicate refinement annotation for one declaration".into(),
                loc: Some(annotation.loc.clone()),
            });
        }
        if let AnnotationTarget::Param {
            function_name,
            function_start,
            ..
        } = &annotation.target
            && !contracts.contains_key(&(function_name.clone(), *function_start))
        {
            errors.push(RtError {
                message: format!(
                    "Refined parameter of '{function_name}' requires a function signature"
                ),
                loc: Some(annotation.loc.clone()),
            });
        }
    }
    errors
}

struct Verifier<'a> {
    source: &'a str,
    file_name: &'a str,
    contracts: HashMap<(String, u32), Contract>,
    signatures: HashMap<String, Contract>,
    library: LibraryRegistry,
    imports: HashMap<String, ImportBinding>,
    top_level_bindings: HashSet<String>,
    has_unmodeled_import_effects: bool,
    compiler_hints: Option<&'a CompilerHints>,
    variable_types: HashMap<u32, (String, RefinementType, SourceLocation)>,
    consumed_variable_types: HashSet<u32>,
    errors: Vec<RtError>,
    fresh: usize,
}

struct CompilerSubexpressionValidator<'verifier, 'source> {
    verifier: &'verifier mut Verifier<'source>,
    state: &'verifier mut State,
    valid: bool,
}

struct LocalImplementationDetector<'state, 'hints, 'contracts> {
    state: &'state State,
    compiler_hints: Option<&'hints CompilerHints>,
    contracts: &'contracts HashMap<String, Contract>,
    found: bool,
}

struct EscapedReferenceCollector<'state> {
    state: &'state State,
    compiler_hints: Option<&'state CompilerHints>,
    terms: Vec<Term>,
}

#[derive(Default)]
struct CompilerReferenceIdentifierCollector {
    identifiers: Vec<(String, Span)>,
}

struct EscapedLocalCallableCollector<'state, 'contracts> {
    state: &'state State,
    contracts: &'contracts HashMap<String, Contract>,
    found: bool,
}

#[derive(Clone, Copy)]
struct CallbackContext<'value> {
    timing: Option<CallbackTiming>,
    receiver: Option<&'value Value>,
    initial_value: Option<&'value Value>,
}

impl<'ast> Visit<'ast> for CompilerReferenceIdentifierCollector {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'ast>) {
        self.identifiers
            .push((identifier.name.to_string(), identifier.span));
    }

    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'ast>) {
        // A nested callee is executed, not passed to the enclosing call. Only
        // bindings occurring in its arguments can escape through it.
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression() {
                self.visit_expression(expression);
            }
        }
    }

    fn visit_new_expression(&mut self, expression: &oxc_ast::ast::NewExpression<'ast>) {
        for argument in &expression.arguments {
            if let Some(expression) = argument.as_expression() {
                self.visit_expression(expression);
            }
        }
    }
}

impl<'ast> Visit<'ast> for EscapedLocalCallableCollector<'_, '_> {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'ast>) {
        if self.contracts.contains_key(identifier.name.as_str()) {
            self.found = true;
            return;
        }
        let Some(value) = self.state.env.get(identifier.name.as_str()) else {
            return;
        };
        let argument_aliases = reference_aliases(self.state, &value.term);
        self.found |= self.contracts.keys().any(|name| {
            let callable = Term::Var(format!("function::{name}"), Sort::Ref);
            let callable_targets = reference_provenance_targets(self.state, &callable);
            !callable_targets.is_disjoint(&argument_aliases)
        });
    }

    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'ast>) {
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression() {
                self.visit_expression(expression);
            }
        }
    }
}

impl<'ast> Visit<'ast> for EscapedReferenceCollector<'_> {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'ast>) {
        if let Some(value) = self
            .state
            .env
            .get(identifier.name.as_str())
            .or_else(|| self.state.compiler_bindings.get(identifier.name.as_str()))
            && value.term.sort() == Sort::Ref
            && !matches!(value.base, BaseType::Function(_, _))
            && base_may_contain_local_implementation(&value.base)
        {
            self.terms.push(value.term.clone());
        }
    }

    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'ast>) {
        // The callee and scalar member receivers are evaluated, not passed to
        // the enclosing call. Only values passed to this nested call escape.
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression() {
                self.visit_expression(expression);
            }
        }
    }

    fn visit_new_expression(&mut self, expression: &oxc_ast::ast::NewExpression<'ast>) {
        for argument in &expression.arguments {
            if let Some(expression) = argument.as_expression() {
                self.visit_expression(expression);
            }
        }
    }

    fn visit_static_member_expression(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'ast>,
    ) {
        if compiler_span_is_escapable_reference(self.compiler_hints, member.span) {
            self.visit_expression(&member.object);
        }
    }

    fn visit_computed_member_expression(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'ast>,
    ) {
        if compiler_span_is_escapable_reference(self.compiler_hints, member.span) {
            self.visit_expression(&member.object);
        }
    }
}

impl LocalImplementationDetector<'_, '_, '_> {
    fn new<'state, 'hints, 'contracts>(
        state: &'state State,
        compiler_hints: Option<&'hints CompilerHints>,
        contracts: &'contracts HashMap<String, Contract>,
    ) -> LocalImplementationDetector<'state, 'hints, 'contracts> {
        LocalImplementationDetector {
            state,
            compiler_hints,
            contracts,
            found: false,
        }
    }
}

impl<'ast> Visit<'ast> for LocalImplementationDetector<'_, '_, '_> {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'ast>) {
        if !self.state.env.contains_key(identifier.name.as_str())
            && let Some(contract) = self.contracts.get(identifier.name.as_str())
        {
            self.found |= base_may_contain_local_implementation(&contract.ret.base);
            return;
        }
        let local_binding = self
            .state
            .env
            .get(identifier.name.as_str())
            .or_else(|| self.state.compiler_bindings.get(identifier.name.as_str()))
            .is_some_and(|value| value.local_implementation);
        let implementation_definition = !self.state.env.contains_key(identifier.name.as_str())
            && !self
                .state
                .compiler_bindings
                .contains_key(identifier.name.as_str())
            && self
                .compiler_hints
                .and_then(|hints| hints.get(identifier.span))
                .is_some_and(|hint| {
                    hint.rendered_type.is_some() && !hint.rendered_type_is_declaration_backed
                });
        self.found |= local_binding || implementation_definition;
    }

    fn visit_object_expression(&mut self, _object: &oxc_ast::ast::ObjectExpression<'ast>) {
        self.found = true;
    }

    fn visit_arrow_function_expression(
        &mut self,
        _arrow: &oxc_ast::ast::ArrowFunctionExpression<'ast>,
    ) {
        self.found = true;
    }

    fn visit_function(
        &mut self,
        _function: &oxc_ast::ast::Function<'ast>,
        _flags: oxc_syntax::scope::ScopeFlags,
    ) {
        self.found = true;
    }

    fn visit_class(&mut self, _class: &oxc_ast::ast::Class<'ast>) {
        self.found = true;
    }
}

impl CompilerSubexpressionValidator<'_, '_> {
    fn reject_refined_callable_escape(&mut self, value: &Value, span: Span) {
        if !base_contains_callable_preconditions(&value.base) {
            return;
        }
        self.verifier.error(
            "A function with refinement preconditions cannot escape to compiler-owned code because TypeScript cannot enforce them"
                .into(),
            span,
        );
        self.valid = false;
    }

    fn reject_type_assertion(&mut self, span: Span) {
        self.verifier.error(
            "Type assertions and non-null assertions cannot provide evidence to refinement checking"
                .into(),
            span,
        );
        self.valid = false;
    }
}

impl<'ast> Visit<'ast> for CompilerSubexpressionValidator<'_, '_> {
    fn visit_ts_as_expression(&mut self, assertion: &oxc_ast::ast::TSAsExpression<'ast>) {
        self.reject_type_assertion(assertion.span);
    }

    fn visit_ts_type_assertion(&mut self, assertion: &oxc_ast::ast::TSTypeAssertion<'ast>) {
        self.reject_type_assertion(assertion.span);
    }

    fn visit_ts_non_null_expression(
        &mut self,
        assertion: &oxc_ast::ast::TSNonNullExpression<'ast>,
    ) {
        self.reject_type_assertion(assertion.span);
    }

    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'ast>) {
        let name = identifier.name.as_str();
        let refined_contract = self
            .verifier
            .signatures
            .get(name)
            .is_some_and(contract_has_callable_preconditions);
        let refined_value = self
            .state
            .env
            .get(name)
            .or_else(|| self.state.compiler_bindings.get(name))
            .is_some_and(|value| base_contains_callable_preconditions(&value.base));
        if refined_contract || refined_value {
            self.verifier.error(
                format!(
                    "Refined function '{name}' cannot escape to compiler-owned code because TypeScript cannot enforce its refinement preconditions"
                ),
                identifier.span,
            );
            self.valid = false;
        }
    }

    fn visit_object_expression(&mut self, object: &oxc_ast::ast::ObjectExpression<'ast>) {
        for property in &object.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    if property.computed
                        && let Some(key) = property.key.as_expression()
                    {
                        self.visit_expression(key);
                    }
                    if property.kind == PropertyKind::Init
                        && !property.method
                        && !matches!(
                            property.value,
                            Expression::ArrowFunctionExpression(_)
                                | Expression::FunctionExpression(_)
                        )
                    {
                        self.visit_expression(&property.value);
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    // Object spread performs observable property reads and may
                    // invoke project-authored getters.
                    self.verifier.cross_unmodeled_execution_boundary(self.state);
                    self.visit_expression(&spread.argument);
                }
            }
        }
    }

    fn visit_arrow_function_expression(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'ast>,
    ) {
        let mut callback_state = self.state.clone();
        self.verifier.havoc_unmodeled_effects(&mut callback_state);
        let errors_before = self.verifier.errors.len();
        let valid = {
            let mut validator = CompilerSubexpressionValidator {
                verifier: self.verifier,
                state: &mut callback_state,
                valid: true,
            };
            oxc_ast_visit::walk::walk_arrow_function_expression(&mut validator, arrow);
            validator.valid
        };
        self.valid &= valid && self.verifier.errors.len() == errors_before;
    }

    fn visit_function(
        &mut self,
        function: &oxc_ast::ast::Function<'ast>,
        flags: oxc_syntax::scope::ScopeFlags,
    ) {
        let mut callback_state = self.state.clone();
        self.verifier.havoc_unmodeled_effects(&mut callback_state);
        let errors_before = self.verifier.errors.len();
        let valid = {
            let mut validator = CompilerSubexpressionValidator {
                verifier: self.verifier,
                state: &mut callback_state,
                valid: true,
            };
            oxc_ast_visit::walk::walk_function(&mut validator, function, flags);
            validator.valid
        };
        self.valid &= valid && self.verifier.errors.len() == errors_before;
    }

    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'ast>) {
        let errors_before = self.verifier.errors.len();
        let value = self.verifier.infer_call(call, self.state);
        if let Some(value) = &value {
            self.reject_refined_callable_escape(value, call.span);
        }
        self.valid &= value.is_some() && self.verifier.errors.len() == errors_before;
    }

    fn visit_static_member_expression(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'ast>,
    ) {
        let errors_before = self.verifier.errors.len();
        let value = self.verifier.infer_static_member(member, self.state);
        if let Some(value) = &value {
            self.reject_refined_callable_escape(value, member.span);
        }
        self.valid &= value.is_some() && self.verifier.errors.len() == errors_before;
    }

    fn visit_computed_member_expression(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'ast>,
    ) {
        let errors_before = self.verifier.errors.len();
        let value = self.verifier.infer_computed_member(member, self.state);
        if let Some(value) = &value {
            self.reject_refined_callable_escape(value, member.span);
        }
        self.valid &= value.is_some() && self.verifier.errors.len() == errors_before;
    }

    fn visit_assignment_expression(
        &mut self,
        assignment: &oxc_ast::ast::AssignmentExpression<'ast>,
    ) {
        self.verifier.error(
            "Assignments inside compiler-backed expressions are outside the supported refinement subset"
                .into(),
            assignment.span,
        );
        self.valid = false;
    }

    fn visit_update_expression(&mut self, update: &oxc_ast::ast::UpdateExpression<'ast>) {
        self.verifier.error(
            "Updates inside compiler-backed expressions are outside the supported refinement subset"
                .into(),
            update.span,
        );
        self.valid = false;
    }
}

impl Verifier<'_> {
    fn verify_program(&mut self, program: &Program<'_>) {
        self.collect_imports(program);
        let mut top_level_functions = HashSet::new();
        let mut top_level_function_names = HashSet::new();
        for statement in &program.body {
            let Statement::FunctionDeclaration(function) = statement else {
                continue;
            };
            let Some(identifier) = &function.id else {
                continue;
            };
            let name = identifier.name.to_string();
            self.top_level_bindings.insert(name.clone());
            if !top_level_function_names.insert(name.clone()) {
                self.error(
                    format!("Duplicate top-level function name '{name}'"),
                    function.span,
                );
                continue;
            }
            if is_reserved_runtime_root(&name) {
                self.error(
                    format!("'{name}' is reserved by the refinement runtime or prelude"),
                    function.span,
                );
                continue;
            }
            let key = (name.clone(), function.span.start);
            top_level_functions.insert(key.clone());
            if let Some(contract) = self.contracts.get(&key).cloned()
                && self.signatures.insert(name.clone(), contract).is_some()
            {
                self.error(
                    format!("Duplicate refined function name '{name}'"),
                    function.span,
                );
            }
        }
        for statement in &program.body {
            if let Statement::FunctionDeclaration(function) = statement {
                self.verify_function(function);
            }
        }
        for ((name, declaration_start), contract) in &self.contracts {
            if !top_level_functions.contains(&(name.clone(), *declaration_start)) {
                self.errors.push(RtError {
                    message: format!(
                        "Refined function '{name}' must be a top-level function declaration"
                    ),
                    loc: Some(contract.loc.clone()),
                });
            }
        }

        let statements: Vec<&Statement<'_>> = program
            .body
            .iter()
            .filter(|statement| {
                !matches!(
                    statement,
                    Statement::FunctionDeclaration(_) | Statement::ImportDeclaration(_)
                )
            })
            .collect();
        let mut initial_state = State::default();
        if self.has_unmodeled_import_effects {
            self.cross_unmodeled_execution_boundary(&mut initial_state);
        }
        let mut states = vec![initial_state];
        for state in &mut states {
            Self::enter_scope(&statements, state);
        }
        for statement in statements {
            states = self.verify_statement(statement, states, None);
        }
        let unconsumed: Vec<_> = self
            .variable_types
            .iter()
            .filter(|(start, _)| !self.consumed_variable_types.contains(start))
            .map(|(_, (name, _, loc))| (name.clone(), loc.clone()))
            .collect();
        for (name, loc) in unconsumed {
            self.errors.push(RtError {
                message: format!("Refined variable '{name}' is outside a statically checked scope"),
                loc: Some(loc),
            });
        }
    }

    fn collect_imports(&mut self, program: &Program<'_>) {
        for statement in &program.body {
            let Statement::ImportDeclaration(declaration) = statement else {
                continue;
            };
            let specifier = declaration.source.value.as_str();
            let module = match self.library.module(specifier) {
                Some(module) => module.specifier.clone(),
                None if self.compiler_hints.is_some() => {
                    self.has_unmodeled_import_effects = true;
                    specifier.to_string()
                }
                None => {
                    // Even a side-effect-only import can replace a built-in
                    // prototype before the entry module starts executing.
                    self.has_unmodeled_import_effects = true;
                    if declaration
                        .specifiers
                        .as_ref()
                        .is_some_and(|specifiers| !specifiers.is_empty())
                    {
                        self.error(
                            format!(
                                "No standard-library declarations for imported module '{specifier}' in the {} environment",
                                self.library.environment()
                            ),
                            declaration.span,
                        );
                    }
                    continue;
                }
            };
            for imported in declaration.specifiers.iter().flatten() {
                use oxc_ast::ast::{ImportDeclarationSpecifier, ModuleExportName};
                let (local, binding) = match imported {
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => (
                        namespace.local.name.to_string(),
                        ImportBinding::Namespace {
                            module: module.clone(),
                        },
                    ),
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => (
                        default.local.name.to_string(),
                        ImportBinding::Export {
                            module: module.clone(),
                            export: "default".into(),
                        },
                    ),
                    ImportDeclarationSpecifier::ImportSpecifier(named) => {
                        let export = match &named.imported {
                            ModuleExportName::IdentifierName(identifier) => {
                                identifier.name.to_string()
                            }
                            ModuleExportName::IdentifierReference(identifier) => {
                                identifier.name.to_string()
                            }
                            ModuleExportName::StringLiteral(literal) => literal.value.to_string(),
                        };
                        (
                            named.local.name.to_string(),
                            ImportBinding::Export {
                                module: module.clone(),
                                export,
                            },
                        )
                    }
                };
                if self.compiler_hints.is_none()
                    && let ImportBinding::Export { module, export } = &binding
                    && self.library.module_export(module, export).is_none()
                {
                    self.error(
                        format!(
                            "No standard-library declaration for export '{export}' from module '{module}' in the {} environment",
                            self.library.environment()
                        ),
                        imported.span(),
                    );
                    continue;
                }
                if self.imports.insert(local.clone(), binding).is_some() {
                    self.error(
                        format!("Duplicate imported binding '{local}'"),
                        imported.span(),
                    );
                }
            }
        }
    }

    fn verify_function(&mut self, function: &Function<'_>) {
        let Some(name) = function.id.as_ref().map(|id| id.name.to_string()) else {
            return;
        };
        let Some(contract) = self
            .contracts
            .get(&(name.clone(), function.span.start))
            .cloned()
        else {
            return;
        };
        let Some(body) = &function.body else { return };
        if function.r#async || function.generator {
            self.error(
                "Async and generator functions are outside the supported refinement subset".into(),
                function.span,
            );
            return;
        }
        if function.params.items.len() != contract.params.len() {
            self.error(
                format!(
                    "Refinement signature for '{name}' declares {} parameters, but JavaScript declares {}",
                    contract.params.len(),
                    function.params.items.len()
                ),
                function.params.span,
            );
            return;
        }
        if function.params.rest.is_some() {
            self.error(
                "Rest parameters are outside the supported refinement subset".into(),
                function.params.span,
            );
            return;
        }
        for (formal, (annotated_name, _)) in function.params.items.iter().zip(&contract.params) {
            if formal.initializer.is_some() {
                self.error(
                    "Default parameters are outside the supported refinement subset".into(),
                    formal.span,
                );
                return;
            }
            let actual_name = match &formal.pattern {
                BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str()),
                _ => None,
            };
            if actual_name != Some(annotated_name.as_str()) {
                self.error(
                    format!(
                        "Refinement parameter '{}' does not match the JavaScript parameter",
                        annotated_name
                    ),
                    formal.span,
                );
                return;
            }
            if actual_name == Some("__rt") {
                self.error(
                    "'__rt' is reserved for refinement runtime assertions".into(),
                    formal.span,
                );
                return;
            }
        }
        let mut state = State::default();
        if self.has_unmodeled_import_effects {
            self.cross_unmodeled_execution_boundary(&mut state);
        }
        let mut replacements = HashMap::new();

        for predicate_name in &contract.predicate_params {
            if !contract
                .params
                .iter()
                .any(|(_, ty)| contains_predicate(ty.predicate.as_ref(), predicate_name))
            {
                self.error(
                    format!(
                        "Predicate parameter '{predicate_name}' must occur in a parameter refinement"
                    ),
                    function.span,
                );
                return;
            }
        }

        let index_names = index_names_in_contract(&contract);
        for (param_name, ty) in &contract.params {
            let sort = if index_names.contains(param_name)
                && matches!(&ty.base, BaseType::Primitive(kind) if kind == "number")
            {
                Sort::Int
            } else {
                sort_for_base(&ty.base)
            };
            let term = Term::Var(format!("{name}.{param_name}"), sort);
            replacements.insert(param_name.clone(), term.clone());
            state.entry_params.insert(param_name.clone(), term.clone());
            state.env.insert(
                param_name.clone(),
                Value {
                    term: term.clone(),
                    base: ty.base.clone(),
                    declared_base: Some(ty.base.clone()),
                    qualifier: None,
                    mutable: false,
                    catalog_trusted: declared_base_has_catalog_identity(&ty.base),
                    local_implementation: base_may_contain_local_implementation(&ty.base),
                },
            );
            state
                .assumptions
                .extend(intrinsic_refinements(&ty.base, &term));
        }
        let mut predicate_replacements = replacements.clone();
        predicate_replacements.insert(
            "$".into(),
            Term::Var(format!("{name}.$return"), sort_for_base(&contract.ret.base)),
        );
        if let Err(message) =
            validate_predicate_parameter_domains(&contract, &predicate_replacements)
        {
            self.error(message, function.span);
            return;
        }
        for (param_name, ty) in &contract.params {
            if let (Some(index), Some(value)) = (&ty.index, state.env.get(param_name).cloned()) {
                match predicate_term(index, &replacements, &HashMap::new(), Some(Sort::Int)) {
                    Ok(index_term) => {
                        let formula = index_formula(&value, &index_term);
                        state.assumptions.push(formula.clone());
                        if let Some(binding) = state.env.get_mut(param_name) {
                            let formula = match binding.qualifier.take() {
                                Some(previous) => {
                                    Term::And(Box::new(previous.formula), Box::new(formula))
                                }
                                None => formula,
                            };
                            binding.qualifier = Some(Qualifier {
                                value: binding.term.clone(),
                                formula,
                            });
                        }
                    }
                    Err(message) => self.error(message, function.span),
                }
            }
            if let Some(predicate) = &ty.predicate {
                match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                    Ok(formula) => {
                        state.assumptions.push(formula.clone());
                        if let Some(value) = state.env.get_mut(param_name) {
                            value.qualifier = Some(Qualifier {
                                value: value.term.clone(),
                                formula,
                            });
                        }
                    }
                    Err(message) => self.error(message, function.span),
                }
            }
        }

        let mut states = vec![state];
        let body_statements: Vec<_> = body.statements.iter().collect();
        for state in &mut states {
            Self::enter_scope(&body_statements, state);
        }
        for statement in &body.statements {
            states = self.verify_statement(statement, states, Some((&name, &contract)));
        }
        if !states.is_empty() && !is_void(&contract.ret.base) {
            self.errors.push(RtError {
                message: format!("Function '{name}' may complete without returning a value"),
                loc: Some(contract.loc),
            });
        }
    }

    fn verify_variable_declaration(
        &mut self,
        declaration: &oxc_ast::ast::VariableDeclaration<'_>,
        states: Vec<State>,
        current_function: Option<(&str, &Contract)>,
    ) -> Vec<State> {
        for declarator in &declaration.declarations {
            if self.variable_types.contains_key(&declarator.span.start) {
                self.consumed_variable_types.insert(declarator.span.start);
            }
        }
        if declaration.kind.is_var() || declaration.kind.is_using() {
            self.error(
                "Only let and const declarations are supported by refinement checking".into(),
                declaration.span,
            );
            return Vec::new();
        }
        let mut output = Vec::new();
        for mut state in states {
            for declarator in &declaration.declarations {
                let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                    self.error(
                        "Destructuring declarations are outside the supported refinement subset"
                            .into(),
                        declarator.id.span(),
                    );
                    continue;
                };
                let annotation = self.variable_types.get(&declarator.span.start).cloned();
                let name = identifier.name.to_string();
                if is_reserved_runtime_root(&name) {
                    self.error(
                        format!("'{name}' is reserved by the refinement runtime or prelude"),
                        declarator.span,
                    );
                    continue;
                }
                if self.signatures.contains_key(&name) {
                    self.error(
                        format!("Declaration '{name}' shadows a refined function signature"),
                        declarator.span,
                    );
                    continue;
                }
                if current_function.is_some_and(|(_, contract)| {
                    contract.params.iter().any(|(param, _)| param == &name)
                }) {
                    self.error(
                        format!(
                            "Declaration shadows refined parameter '{name}', which is not supported"
                        ),
                        declarator.span,
                    );
                    continue;
                }
                if !Self::initialize_name(&name, &mut state) {
                    self.error(
                        format!("Duplicate declaration of '{name}' in one scope"),
                        declarator.span,
                    );
                    continue;
                }
                let Some(initializer) = &declarator.init else {
                    let loc = annotation
                        .map(|(_, _, loc)| loc)
                        .unwrap_or_else(|| self.location(declarator.span));
                    self.errors.push(RtError {
                        message: format!(
                            "Variable '{name}' requires an initializer in refinement checking"
                        ),
                        loc: Some(loc),
                    });
                    continue;
                };
                if let Some(mut value) = self.infer_expression(initializer, &mut state) {
                    value.mutable = !declaration.kind.is_const();
                    let mut declared_predicate = None;
                    if let Some((annotated_name, annotation, loc)) = annotation {
                        debug_assert_eq!(annotated_name, name);
                        self.check_base(&value.base, &annotation.base, initializer.span());
                        value.base = annotation.base.clone();
                        value.declared_base = Some(annotation.base.clone());
                        let replacements = HashMap::from([(name.clone(), value.term.clone())]);
                        if let Some(index) = &annotation.index {
                            self.prove_index(
                                &value,
                                index,
                                &replacements,
                                &state.assumptions,
                                format!("Initializer for '{name}' does not match its index"),
                                loc.clone(),
                                declarator.span,
                            );
                        }
                        if let Some(predicate) = &annotation.predicate {
                            match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                                Ok(goal) => {
                                    self.prove(
                                        &state.assumptions,
                                        &goal,
                                        format!("Initializer for '{name}' does not satisfy its refinement"),
                                        loc,
                                    );
                                    declared_predicate = Some(predicate.clone());
                                }
                                Err(message) => self.error(message, declarator.span),
                            }
                        }
                    }
                    if declared_predicate.is_some() {
                        value.qualifier = None;
                    }
                    self.bind_value(&name, value, &mut state);
                    if let Some(predicate) = declared_predicate {
                        let symbol = state.env[&name].term.clone();
                        let replacements = HashMap::from([(name.clone(), symbol.clone())]);
                        match predicate_term(&predicate, &replacements, &HashMap::new(), None) {
                            Ok(formula) => {
                                state.assumptions.push(formula.clone());
                                state.env.get_mut(&name).unwrap().qualifier = Some(Qualifier {
                                    value: symbol,
                                    formula,
                                });
                            }
                            Err(message) => self.error(message, declarator.span),
                        }
                    }
                }
            }
            output.push(state);
        }
        output
    }

    fn verify_statement<'a>(
        &mut self,
        statement: &'a Statement<'a>,
        states: Vec<State>,
        current_function: Option<(&str, &Contract)>,
    ) -> Vec<State> {
        match statement {
            Statement::BlockStatement(block) => {
                let mut current = states;
                let block_statements: Vec<_> = block.body.iter().collect();
                for state in &mut current {
                    Self::enter_scope(&block_statements, state);
                }
                for statement in &block.body {
                    current = self.verify_statement(statement, current, current_function);
                }
                for state in &mut current {
                    Self::leave_scope(state);
                }
                current
            }
            Statement::VariableDeclaration(declaration) => {
                return self.verify_variable_declaration(declaration, states, current_function);
                #[allow(unreachable_code)]
                for declarator in &declaration.declarations {
                    if self.variable_types.contains_key(&declarator.span.start) {
                        self.consumed_variable_types.insert(declarator.span.start);
                    }
                }
                if declaration.kind.is_var() || declaration.kind.is_using() {
                    self.error(
                        "Only let and const declarations are supported by refinement checking"
                            .into(),
                        declaration.span,
                    );
                    return Vec::new();
                }
                let mut output = Vec::new();
                for mut state in states {
                    for declarator in &declaration.declarations {
                        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                            self.error(
                                "Destructuring declarations are outside the supported refinement subset"
                                    .into(),
                                declarator.id.span(),
                            );
                            continue;
                        };
                        let annotation = self.variable_types.get(&declarator.span.start).cloned();
                        let name = identifier.name.to_string();
                        if is_reserved_runtime_root(&name) {
                            self.error(
                                format!(
                                    "'{name}' is reserved by the refinement runtime or prelude"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if self.signatures.contains_key(&name) {
                            self.error(
                                format!(
                                    "Declaration '{name}' shadows a refined function signature"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if current_function.is_some_and(|(_, contract)| {
                            contract.params.iter().any(|(param, _)| param == &name)
                        }) {
                            self.error(
                                format!(
                                    "Declaration shadows refined parameter '{name}', which is not supported"
                                ),
                                declarator.span,
                            );
                            continue;
                        }
                        if !Self::initialize_name(&name, &mut state) {
                            self.error(
                                format!("Duplicate declaration of '{name}' in one scope"),
                                declarator.span,
                            );
                            continue;
                        }
                        let Some(initializer) = &declarator.init else {
                            let loc = annotation
                                .map(|(_, _, loc)| loc)
                                .unwrap_or_else(|| self.location(declarator.span));
                            self.errors.push(RtError {
                                message: format!(
                                    "Variable '{name}' requires an initializer in refinement checking"
                                ),
                                loc: Some(loc),
                            });
                            continue;
                        };
                        if let Some(mut value) = self.infer_expression(initializer, &mut state) {
                            value.mutable = !declaration.kind.is_const();
                            let mut declared_predicate = None;
                            if let Some((annotated_name, annotation, loc)) = annotation {
                                debug_assert_eq!(annotated_name, name);
                                self.check_base(&value.base, &annotation.base, initializer.span());
                                value.base = annotation.base.clone();
                                value.declared_base = Some(annotation.base.clone());
                                if let Some(predicate) = &annotation.predicate {
                                    let replacements =
                                        HashMap::from([(name.clone(), value.term.clone())]);
                                    match predicate_term(
                                        predicate,
                                        &replacements,
                                        &HashMap::new(),
                                        None,
                                    ) {
                                        Ok(goal) => {
                                            self.prove(
                                                &state.assumptions,
                                                &goal,
                                                format!("Initializer for '{name}' does not satisfy its refinement"),
                                                loc,
                                            );
                                            declared_predicate = Some(predicate.clone());
                                        }
                                        Err(message) => self.error(message, declarator.span),
                                    }
                                }
                            }
                            if declared_predicate.is_some() {
                                value.qualifier = None;
                            }
                            self.bind_value(&name, value, &mut state);
                            if let Some(predicate) = declared_predicate {
                                let symbol = state.env[&name].term.clone();
                                let replacements = HashMap::from([(name.clone(), symbol.clone())]);
                                match predicate_term(
                                    &predicate,
                                    &replacements,
                                    &HashMap::new(),
                                    None,
                                ) {
                                    Ok(formula) => {
                                        state.assumptions.push(formula.clone());
                                        state.env.get_mut(&name).unwrap().qualifier =
                                            Some(Qualifier {
                                                value: symbol,
                                                formula,
                                            });
                                    }
                                    Err(message) => self.error(message, declarator.span),
                                }
                            }
                        }
                    }
                    output.push(state);
                }
                output
            }
            Statement::ExpressionStatement(expression_statement) => {
                let mut output = Vec::new();
                for mut state in states {
                    if let Expression::AssignmentExpression(assignment) =
                        &expression_statement.expression
                    {
                        if matches!(
                            assignment.operator,
                            AssignmentOperator::Assign
                                | AssignmentOperator::Addition
                                | AssignmentOperator::Subtraction
                        ) {
                            if let Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(
                                identifier,
                            )) = assignment.left.as_simple_assignment_target()
                            {
                                let name = identifier.name.as_str();
                                if current_function.is_some_and(|(_, contract)| {
                                    contract.params.iter().any(|(param, _)| param == name)
                                }) {
                                    self.error(
                                        format!("Reassignment of refined parameter '{name}' is not supported"),
                                        assignment.span,
                                    );
                                    output.push(state);
                                    continue;
                                }
                                let Some(previous) = state.env.get(name).cloned() else {
                                    self.error(
                                        format!("Assignment to untracked variable '{name}'"),
                                        identifier.span,
                                    );
                                    output.push(state);
                                    continue;
                                };
                                if !previous.mutable {
                                    self.error(
                                        format!("Assignment to immutable binding '{name}'"),
                                        assignment.span,
                                    );
                                    output.push(state);
                                    continue;
                                }
                                if let Some(mut value) =
                                    self.infer_expression(&assignment.right, &mut state)
                                {
                                    if assignment.operator != AssignmentOperator::Assign {
                                        let combined = match assignment.operator {
                                            AssignmentOperator::Addition => binary_term(
                                                BinaryOperator::Addition,
                                                previous.term.clone(),
                                                value.term.clone(),
                                            ),
                                            AssignmentOperator::Subtraction => binary_term(
                                                BinaryOperator::Subtraction,
                                                previous.term.clone(),
                                                value.term.clone(),
                                            ),
                                            _ => Err("Unsupported compound assignment".into()),
                                        };
                                        match combined {
                                            Ok(term) => {
                                                value.term = term;
                                                value.base = base_for_sort(value.term.sort());
                                                value.qualifier = None;
                                            }
                                            Err(message) => {
                                                self.error(message, assignment.span);
                                                output.push(state);
                                                continue;
                                            }
                                        }
                                    }
                                    if let Some(expected) = &previous.declared_base {
                                        self.check_base(
                                            &value.base,
                                            expected,
                                            assignment.right.span(),
                                        );
                                        value.declared_base = Some(expected.clone());
                                    }
                                    value.mutable = previous.mutable;
                                    self.bind_value(name, value, &mut state);
                                }
                            } else {
                                self.error(
                                    "Only identifier assignment is supported by refinement checking".into(),
                                    assignment.left.span(),
                                );
                            }
                        } else {
                            self.error(
                                "Compound assignment is outside the supported static refinement subset".into(),
                                assignment.span,
                            );
                        }
                    } else {
                        self.infer_expression(&expression_statement.expression, &mut state);
                    }
                    output.push(state);
                }
                output
            }
            Statement::IfStatement(if_statement) => {
                let mut output = Vec::new();
                for mut state in states {
                    let Some(test) = self.infer_expression(&if_statement.test, &mut state) else {
                        continue;
                    };
                    if value_sort(&test) != Some(Sort::Bool) {
                        self.error(
                            "if condition must be boolean".into(),
                            if_statement.test.span(),
                        );
                        continue;
                    }
                    let mut then_state = state.clone();
                    then_state.assumptions.push(test.term.clone());
                    Self::narrow_qualifiers(&mut then_state, &test.term);
                    if Self::path_is_reachable(&then_state) {
                        output.extend(self.verify_statement(
                            &if_statement.consequent,
                            vec![then_state],
                            current_function,
                        ));
                    }

                    let mut else_state = state;
                    let negated = Term::Not(Box::new(test.term));
                    else_state.assumptions.push(negated.clone());
                    Self::narrow_qualifiers(&mut else_state, &negated);
                    if Self::path_is_reachable(&else_state) {
                        if let Some(alternate) = &if_statement.alternate {
                            output.extend(self.verify_statement(
                                alternate,
                                vec![else_state],
                                current_function,
                            ));
                        } else {
                            output.push(else_state);
                        }
                    }
                }
                output
            }
            Statement::ReturnStatement(return_statement) => {
                let Some((function_name, contract)) = current_function else {
                    return Vec::new();
                };
                for mut state in states {
                    let value = return_statement
                        .argument
                        .as_ref()
                        .and_then(|argument| self.infer_expression(argument, &mut state));
                    let Some(value) = value else {
                        if !is_void(&contract.ret.base) {
                            self.error(
                                format!("Function '{function_name}' returns no value"),
                                return_statement.span,
                            );
                        }
                        continue;
                    };
                    self.check_base(&value.base, &contract.ret.base, return_statement.span);
                    if base_requires_catalog_identity(&contract.ret.base) && !value.catalog_trusted
                    {
                        self.error(
                            format!(
                                "Return value of '{function_name}' does not have a verified standard-library identity"
                            ),
                            return_statement.span,
                        );
                    }
                    let mut replacements = HashMap::from([("$".to_string(), value.term.clone())]);
                    for (param_name, term) in &state.entry_params {
                        replacements.insert(param_name.clone(), term.clone());
                    }
                    if let Some(index) = &contract.ret.index {
                        self.prove_index(
                            &value,
                            index,
                            &replacements,
                            &state.assumptions,
                            format!("Return value of '{function_name}' does not match its index"),
                            contract.loc.clone(),
                            return_statement.span,
                        );
                    }
                    if let Some(predicate) = &contract.ret.predicate {
                        match predicate_term(
                            predicate,
                            &replacements,
                            &HashMap::new(),
                            Some(value.term.sort()),
                        ) {
                            Ok(goal) => self.prove(
                                &state.assumptions,
                                &goal,
                                format!("Return value of '{function_name}' does not satisfy its refinement"),
                                contract.loc.clone(),
                            ),
                            Err(message) => self.error(message, return_statement.span),
                        }
                    }
                }
                Vec::new()
            }
            Statement::EmptyStatement(_) => states,
            Statement::WhileStatement(while_statement) => self.verify_loop(
                Some(&while_statement.test),
                None,
                &while_statement.body,
                while_statement.span,
                states,
                current_function,
            ),
            Statement::ForStatement(for_statement) => {
                let mut current = states;
                if let Some(init) = &for_statement.init {
                    match init {
                        oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration) => {
                            current = self.verify_variable_declaration(
                                declaration,
                                current,
                                current_function,
                            );
                        }
                        other => {
                            for state in &mut current {
                                if let Some(expression) = other.as_expression() {
                                    self.infer_expression(expression, state);
                                } else {
                                    self.error(
                                        "Unsupported for-loop initializer".into(),
                                        for_statement.span,
                                    );
                                }
                            }
                        }
                    }
                }
                self.verify_loop(
                    for_statement.test.as_ref(),
                    for_statement.update.as_ref(),
                    &for_statement.body,
                    for_statement.span,
                    current,
                    current_function,
                )
            }
            _ => {
                self.error(
                    "Statement is outside the supported static refinement subset".into(),
                    statement.span(),
                );
                Vec::new()
            }
        }
    }

    fn infer_expression(
        &mut self,
        expression: &Expression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        match expression {
            Expression::NumericLiteral(literal) => {
                if literal.value.fract() != 0.0 || literal.value.abs() > 9_007_199_254_740_991_f64 {
                    self.error(
                        "Only safe integer literals are supported by refinement checking".into(),
                        literal.span,
                    );
                    return None;
                }
                let term = Term::Int(literal.value as i64);
                let placeholder = Term::Var(self.fresh_name("literal"), Sort::Int);
                Some(Value {
                    term: term.clone(),
                    base: number_type(),
                    declared_base: None,
                    qualifier: Some(Qualifier {
                        value: placeholder.clone(),
                        formula: Term::Eq(Box::new(placeholder), Box::new(term)),
                    }),
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::BooleanLiteral(literal) => {
                let term = Term::Bool(literal.value);
                let placeholder = Term::Var(self.fresh_name("literal"), Sort::Bool);
                Some(Value {
                    term: term.clone(),
                    base: boolean_type(),
                    declared_base: None,
                    qualifier: Some(Qualifier {
                        value: placeholder.clone(),
                        formula: Term::Eq(Box::new(placeholder), Box::new(term)),
                    }),
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::StringLiteral(literal) => {
                let term = Term::String(literal.value.to_string());
                let placeholder = Term::Var(self.fresh_name("literal"), Sort::String);
                let length = collection_length(&term);
                let formula = Term::And(
                    Box::new(Term::Eq(
                        Box::new(placeholder.clone()),
                        Box::new(term.clone()),
                    )),
                    Box::new(Term::Eq(
                        Box::new(length),
                        Box::new(Term::Int(literal.value.encode_utf16().count() as i64)),
                    )),
                );
                Some(Value {
                    term,
                    base: BaseType::Primitive("string".into()),
                    declared_base: None,
                    qualifier: Some(Qualifier {
                        value: placeholder,
                        formula,
                    }),
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::ArrayExpression(array) => self.infer_array(array, state),
            Expression::Identifier(identifier) => {
                let name = identifier.name.as_str();
                if state.uninitialized.contains(name) {
                    self.error(
                        format!("Binding '{name}' is used before its declaration"),
                        identifier.span,
                    );
                    return None;
                }
                if let Some(value) = state.env.get(name).cloned() {
                    return Some(value);
                }
                if let Some(contract) = self.signatures.get(name) {
                    return Some(Value {
                        term: Term::Var(format!("function::{name}"), Sort::Ref),
                        base: contract_function_type(contract),
                        declared_base: None,
                        qualifier: None,
                        mutable: false,
                        catalog_trusted: true,
                        local_implementation: base_may_contain_local_implementation(
                            &contract.ret.base,
                        ),
                    });
                }
                if self.top_level_bindings.contains(name) {
                    let errors_before = self.errors.len();
                    if let Some(value) =
                        self.compiler_identifier_value(name, identifier.span, state)
                    {
                        return Some(value);
                    }
                    if self.errors.len() > errors_before {
                        return None;
                    }
                    self.error(
                        format!("No refinement signature for top-level function '{name}'"),
                        identifier.span,
                    );
                    return None;
                }
                let is_import = self.imports.contains_key(name);
                if let Some(value) = self.imported_value(name, state) {
                    return Some(value);
                }
                if is_import {
                    let errors_before = self.errors.len();
                    if let Some(value) =
                        self.compiler_identifier_value(name, identifier.span, state)
                    {
                        return Some(value);
                    }
                    if self.errors.len() > errors_before {
                        return None;
                    }
                    self.error(
                        format!("No static type information for imported binding '{name}'"),
                        identifier.span,
                    );
                    return None;
                }
                if let Some(ty) = self.library.global(name).cloned() {
                    return Some(self.ambient_value(name, &ty, state));
                }
                let errors_before = self.errors.len();
                if let Some(value) = self.compiler_identifier_value(name, identifier.span, state) {
                    return Some(value);
                }
                if self.errors.len() > errors_before {
                    return None;
                }
                self.error(
                    format!("No static type information for '{}'", identifier.name),
                    identifier.span,
                );
                None
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.infer_expression(&parenthesized.expression, state)
            }
            Expression::SequenceExpression(sequence) => {
                let mut result = None;
                for expression in &sequence.expressions {
                    result = Some(self.infer_expression(expression, state)?);
                }
                result
            }
            Expression::UnaryExpression(unary) => {
                let value = self.infer_expression(&unary.argument, state)?;
                let Some(value_sort) = value_sort(&value) else {
                    self.error(
                        "Unary operators require a number or boolean operand in refinement checking"
                            .into(),
                        unary.span,
                    );
                    return None;
                };
                let term = match unary.operator {
                    UnaryOperator::LogicalNot if value_sort == Sort::Bool => {
                        Term::Not(Box::new(value.term))
                    }
                    UnaryOperator::UnaryNegation
                        if matches!(value.term.sort(), Sort::Number | Sort::Int) =>
                    {
                        let zero = if value.term.sort() == Sort::Int {
                            Term::Int(0)
                        } else {
                            Term::Number(0)
                        };
                        Term::Sub(Box::new(zero), Box::new(value.term))
                    }
                    UnaryOperator::UnaryPlus
                        if matches!(value.term.sort(), Sort::Number | Sort::Int) =>
                    {
                        value.term
                    }
                    _ => {
                        self.error(
                            "Unsupported unary expression in refinement analysis".into(),
                            unary.span,
                        );
                        return None;
                    }
                };
                Some(Value {
                    base: base_for_sort(term.sort()),
                    term,
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::BinaryExpression(binary) => {
                let left = self.infer_expression(&binary.left, state)?;
                let right = self.infer_expression(&binary.right, state)?;
                if value_sort(&left).is_none() || value_sort(&right).is_none() {
                    self.error(
                        "Binary operators require number or boolean operands in refinement checking"
                            .into(),
                        binary.span,
                    );
                    return None;
                }
                let term = binary_term(binary.operator, left.term, right.term)
                    .map_err(|message| {
                        self.error(message, binary.span);
                    })
                    .ok()?;
                Some(Value {
                    base: base_for_sort(term.sort()),
                    term,
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::LogicalExpression(logical) => {
                let left = self.infer_expression(&logical.left, state)?;
                let right = self.infer_expression(&logical.right, state)?;
                if value_sort(&left) != Some(Sort::Bool) || value_sort(&right) != Some(Sort::Bool) {
                    self.error(
                        "Logical expressions require boolean operands in refinement checking"
                            .into(),
                        logical.span,
                    );
                    return None;
                }
                let term = match logical.operator {
                    LogicalOperator::And => Term::And(Box::new(left.term), Box::new(right.term)),
                    LogicalOperator::Or => Term::Or(Box::new(left.term), Box::new(right.term)),
                    LogicalOperator::Coalesce => {
                        self.error(
                            "Nullish coalescing is not supported in refinements".into(),
                            logical.span,
                        );
                        return None;
                    }
                };
                Some(Value {
                    term,
                    base: boolean_type(),
                    declared_base: None,
                    qualifier: None,
                    mutable: true,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
            Expression::AssignmentExpression(assignment) => {
                self.infer_assignment_expression(assignment, state)
            }
            Expression::UpdateExpression(update) => self.infer_update(update, state),
            Expression::CallExpression(call) => self.infer_call(call, state),
            Expression::StaticMemberExpression(member) => self.infer_static_member(member, state),
            Expression::ComputedMemberExpression(member) => {
                self.infer_computed_member(member, state)
            }
            _ => {
                let errors_before = self.errors.len();
                if let Some(value) = self.compiler_expression_value(expression, state) {
                    return Some(value);
                }
                if self.errors.len() > errors_before {
                    return None;
                }
                self.error(
                    "Expression is outside the supported static refinement subset".into(),
                    expression.span(),
                );
                None
            }
        }
    }

    fn infer_assignment_expression(
        &mut self,
        assignment: &oxc_ast::ast::AssignmentExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        if !matches!(
            assignment.operator,
            AssignmentOperator::Assign
                | AssignmentOperator::Addition
                | AssignmentOperator::Subtraction
        ) {
            self.error(
                "Compound assignment is outside the supported static refinement subset".into(),
                assignment.span,
            );
            return None;
        }
        let Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier)) =
            assignment.left.as_simple_assignment_target()
        else {
            self.error(
                "Only identifier assignment is supported by refinement checking".into(),
                assignment.left.span(),
            );
            return None;
        };
        let name = identifier.name.as_str();
        let Some(previous) = state.env.get(name).cloned() else {
            self.error(
                format!("Assignment to untracked variable '{name}'"),
                identifier.span,
            );
            return None;
        };
        if !previous.mutable {
            self.error(
                format!("Assignment to immutable binding '{name}'"),
                assignment.span,
            );
            return None;
        }
        let mut value = self.infer_expression(&assignment.right, state)?;
        if assignment.operator != AssignmentOperator::Assign {
            let combined = match assignment.operator {
                AssignmentOperator::Addition => binary_term(
                    BinaryOperator::Addition,
                    previous.term.clone(),
                    value.term.clone(),
                ),
                AssignmentOperator::Subtraction => binary_term(
                    BinaryOperator::Subtraction,
                    previous.term.clone(),
                    value.term.clone(),
                ),
                _ => Err("Unsupported compound assignment".into()),
            };
            match combined {
                Ok(term) => {
                    value.term = term;
                    value.base = base_for_sort(value.term.sort());
                    value.qualifier = None;
                }
                Err(message) => {
                    self.error(message, assignment.span);
                    return None;
                }
            }
        }
        if let Some(expected) = &previous.declared_base {
            self.check_base(&value.base, expected, assignment.right.span());
            value.declared_base = Some(expected.clone());
        }
        value.mutable = previous.mutable;
        let result = value.clone();
        self.bind_value(name, value, state);
        Some(result)
    }

    fn infer_update(
        &mut self,
        update: &oxc_ast::ast::UpdateExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        let SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) = &update.argument
        else {
            self.error(
                "Only identifier increment/decrement is supported by refinement checking".into(),
                update.span,
            );
            return None;
        };
        let name = identifier.name.as_str();
        let Some(previous) = state.env.get(name).cloned() else {
            self.error(
                format!("Update of untracked variable '{name}'"),
                identifier.span,
            );
            return None;
        };
        if !previous.mutable {
            self.error(format!("Update of immutable binding '{name}'"), update.span);
            return None;
        }
        if !is_numeric_sort(previous.term.sort()) {
            self.error(
                "Increment and decrement require a numeric binding".into(),
                update.span,
            );
            return None;
        }
        let one = one_for(&previous.term);
        let operator = match update.operator {
            UpdateOperator::Increment => BinaryOperator::Addition,
            UpdateOperator::Decrement => BinaryOperator::Subtraction,
        };
        let next = match binary_term(operator, previous.term.clone(), one) {
            Ok(term) => term,
            Err(message) => {
                self.error(message, update.span);
                return None;
            }
        };
        let result = if update.prefix {
            next.clone()
        } else {
            previous.term.clone()
        };
        let mut value = previous.clone();
        value.term = next;
        value.qualifier = None;
        self.bind_value(name, value, state);
        Some(Value {
            term: result,
            base: number_type(),
            declared_base: None,
            qualifier: None,
            mutable: true,
            catalog_trusted: true,
            local_implementation: false,
        })
    }

    fn infer_array(
        &mut self,
        array: &oxc_ast::ast::ArrayExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        let reference = Term::Var(self.fresh_name("array"), Sort::Ref);
        let mut element_types = Vec::new();
        let mut local_implementation = false;
        let mut facts = vec![Term::Eq(
            Box::new(collection_length(&reference)),
            Box::new(Term::Int(array.elements.len() as i64)),
        )];

        for (index, element) in array.elements.iter().enumerate() {
            let Some(expression) = element.as_expression() else {
                self.error(
                    "Array holes and spread elements are outside the supported refinement subset"
                        .into(),
                    element.span(),
                );
                return None;
            };
            let value = self.infer_expression(expression, state)?;
            if let Some(qualifier) = &value.qualifier {
                state.assumptions.push(qualifier.formula.clone());
            }
            local_implementation |= value.local_implementation;
            element_types.push(value.base.clone());
            facts.push(Term::Same(
                Box::new(Term::Index(
                    Box::new(reference.clone()),
                    Box::new(Term::Int(index as i64)),
                    sort_for_base(&value.base),
                )),
                Box::new(value.term),
            ));
        }

        let element = normalize_union(element_types);
        let formula = and_terms(facts);
        record_reference_provenance_edges(&formula, &mut state.provenance_edges);
        Some(Value {
            term: reference.clone(),
            base: BaseType::Generic("DenseArray".into(), vec![element]),
            declared_base: None,
            qualifier: Some(Qualifier {
                value: reference,
                formula,
            }),
            mutable: true,
            catalog_trusted: true,
            local_implementation,
        })
    }

    fn imported_value(&mut self, name: &str, state: &mut State) -> Option<Value> {
        let ImportBinding::Export { module, export } = self.imports.get(name)?.clone() else {
            return Some(self.ambient_value(
                &format!("import::{name}"),
                &RefinementType::from_base(BaseType::Named("ModuleNamespace".into())),
                state,
            ));
        };
        match self.library.module_export(&module, &export).cloned()? {
            LibraryExport::Value(ty) => {
                Some(self.ambient_value(&format!("{module}.{export}"), &ty, state))
            }
            LibraryExport::Function(overloads) => {
                let signature = overloads.first()?;
                Some(Value {
                    term: Term::Var(format!("function::{module}.{export}"), Sort::Ref),
                    base: library_function_type(signature, &HashMap::new()),
                    declared_base: None,
                    qualifier: None,
                    mutable: false,
                    catalog_trusted: true,
                    local_implementation: false,
                })
            }
        }
    }

    fn ambient_value(&mut self, name: &str, ty: &RefinementType, state: &mut State) -> Value {
        let term = Term::Var(format!("ambient::{name}"), sort_for_base(&ty.base));
        state
            .assumptions
            .extend(intrinsic_refinements(&ty.base, &term));
        let qualifier = ty.predicate.as_ref().and_then(|predicate| {
            let replacements = HashMap::from([("$".to_string(), term.clone())]);
            match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                Ok(formula) => {
                    state.assumptions.push(formula.clone());
                    Some(Qualifier {
                        value: term.clone(),
                        formula,
                    })
                }
                Err(message) => {
                    self.errors.push(RtError { message, loc: None });
                    None
                }
            }
        });
        Value {
            term,
            base: ty.base.clone(),
            declared_base: None,
            qualifier,
            mutable: false,
            catalog_trusted: true,
            local_implementation: false,
        }
    }

    fn known_expression_base(
        &self,
        expression: &Expression<'_>,
        state: &State,
    ) -> Option<BaseType> {
        match expression {
            Expression::Identifier(identifier) => state
                .env
                .get(identifier.name.as_str())
                .or_else(|| state.compiler_bindings.get(identifier.name.as_str()))
                .map(|value| value.base.clone())
                .or_else(|| {
                    self.signatures
                        .get(identifier.name.as_str())
                        .map(contract_function_type)
                }),
            Expression::StaticMemberExpression(member) => {
                let object = self.known_expression_base(&member.object, state)?;
                known_member_base(&object, member.property.name.as_str())
            }
            Expression::ComputedMemberExpression(member) => {
                let object = self.known_expression_base(&member.object, state)?;
                match &member.expression {
                    Expression::StringLiteral(property) => {
                        known_member_base(&object, property.value.as_str())
                    }
                    Expression::NumericLiteral(_) => known_index_base(&object),
                    _ => None,
                }
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.known_expression_base(&parenthesized.expression, state)
            }
            Expression::TSAsExpression(assertion) => {
                self.known_expression_base(&assertion.expression, state)
            }
            Expression::TSSatisfiesExpression(assertion) => {
                self.known_expression_base(&assertion.expression, state)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.known_expression_base(&non_null.expression, state)
            }
            Expression::TSInstantiationExpression(instantiation) => {
                self.known_expression_base(&instantiation.expression, state)
            }
            _ => None,
        }
    }

    fn compiler_base(&self, span: Span, prefer_call_return: bool) -> Option<BaseType> {
        let hint = self.compiler_hints?.get(span)?;
        if prefer_call_return && !hint.call_return_types.is_empty() {
            return Some(normalize_union(
                hint.call_return_types
                    .iter()
                    .map(|rendered| parse_typescript_type(rendered))
                    .collect(),
            ));
        }
        if let Some(rendered_type) = hint.rendered_type.as_deref() {
            return Some(parse_typescript_type(rendered_type));
        }
        None
    }

    fn compiler_value_from_base(
        &mut self,
        base: BaseType,
        label: &str,
        local_implementation: bool,
        state: &mut State,
    ) -> Value {
        let term = Term::Var(self.fresh_name(label), sort_for_base(&base));
        let catalog_trusted = !local_implementation && base_has_unambiguous_catalog_identity(&base);
        if catalog_trusted {
            state
                .assumptions
                .extend(intrinsic_refinements(&base, &term));
        }
        Value {
            term,
            base,
            declared_base: None,
            qualifier: None,
            mutable: true,
            catalog_trusted,
            local_implementation,
        }
    }

    fn compiler_identifier_value(
        &mut self,
        name: &str,
        span: Span,
        state: &mut State,
    ) -> Option<Value> {
        if let Some(value) = state.compiler_bindings.get(name) {
            return Some(value.clone());
        }
        let base = self.compiler_base(span, false)?;
        if !self
            .compiler_hints
            .and_then(|hints| hints.get(span))
            .is_some_and(|hint| hint.rendered_type_is_declaration_backed)
        {
            self.error(
                "Compiler-backed identifier evidence must resolve to a declaration file".into(),
                span,
            );
            return None;
        }
        let local_implementation =
            !state.library_semantics_intact && base_may_contain_local_implementation(&base);
        let assumption_count = state.assumptions.len();
        let mut value =
            self.compiler_value_from_base(base, "compiler_value", local_implementation, state);
        if value.term.sort() == Sort::Ref {
            state.assumptions.truncate(assumption_count);
            value.term = Term::Var(format!("compiler_binding::{name}"), Sort::Ref);
            if value.catalog_trusted {
                state
                    .assumptions
                    .extend(intrinsic_refinements(&value.base, &value.term));
            }
            state
                .compiler_bindings
                .insert(name.to_string(), value.clone());
        }
        Some(value)
    }

    fn compiler_member_value(
        &mut self,
        span: Span,
        receiver_has_local_implementation: bool,
        state: &mut State,
    ) -> Option<Value> {
        let base = self.compiler_base(span, false)?;
        if receiver_has_local_implementation {
            self.error(
                "Compiler-backed member evidence cannot validate a locally implemented object; add an explicit refinement contract"
                    .into(),
                span,
            );
            return None;
        }
        if !self
            .compiler_hints
            .and_then(|hints| hints.get(span))
            .is_some_and(|hint| hint.rendered_type_is_declaration_backed)
        {
            self.error(
                "Compiler-backed member evidence must resolve to declaration files; project implementation members need an explicit refinement contract"
                    .into(),
                span,
            );
            return None;
        }
        self.cross_unmodeled_execution_boundary(state);
        Some(self.compiler_value_from_base(base, "compiler_member", false, state))
    }

    fn compiler_call_value(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        receiver_has_local_implementation: bool,
        state: &mut State,
    ) -> Option<Value> {
        let base = self.compiler_base(call.span, true)?;
        let mut callee_detector =
            LocalImplementationDetector::new(state, self.compiler_hints, &self.signatures);
        callee_detector.visit_expression(&call.callee);
        if receiver_has_local_implementation || callee_detector.found {
            self.error(
                "Compiler-backed call evidence cannot validate a locally implemented object; add an explicit refinement contract"
                    .into(),
                call.callee.span(),
            );
            return None;
        }
        if !self
            .compiler_hints
            .and_then(|hints| hints.get(call.span))
            .is_some_and(|hint| hint.call_is_declaration_backed)
        {
            self.error(
                "Compiler-backed call evidence must resolve to declaration files; project implementation callables need an explicit refinement contract"
                    .into(),
                call.callee.span(),
            );
            return None;
        }
        let mut detector =
            LocalImplementationDetector::new(state, self.compiler_hints, &self.signatures);
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression() {
                detector.visit_expression(expression);
            }
        }
        if detector.found {
            self.error(
                "Locally implemented values cannot flow into compiler-backed calls whose return type is used as refinement evidence"
                    .into(),
                call.span,
            );
            return None;
        }
        if !self.validate_compiler_call_arguments(call, state) {
            return None;
        }
        self.materialize_compiler_argument_bindings(call, state);
        let mut escaped_references = Vec::new();
        let mut escaped_local_callable = EscapedLocalCallableCollector {
            state,
            contracts: &self.signatures,
            found: false,
        };
        for argument in &call.arguments {
            let Some(expression) = argument.as_expression() else {
                continue;
            };
            escaped_local_callable.visit_expression(expression);
            let mut collector = EscapedReferenceCollector {
                state,
                compiler_hints: self.compiler_hints,
                terms: Vec::new(),
            };
            collector.visit_expression(expression);
            escaped_references.extend(collector.terms);
        }
        let result_may_alias_local_implementation = (escaped_local_callable.found
            || !escaped_references.is_empty())
            && base_may_contain_local_implementation(&base);
        let escaped_local_callable = escaped_local_callable.found;
        self.cross_unmodeled_execution_boundary(state);
        if escaped_local_callable {
            taint_implementation_capable_references(state);
        }
        for reference in escaped_references {
            mark_reference_aliases_as_local(state, &reference);
        }
        Some(self.compiler_value_from_base(
            base,
            "compiler_call",
            result_may_alias_local_implementation,
            state,
        ))
    }

    fn materialize_compiler_argument_bindings(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        state: &mut State,
    ) {
        let mut collector = CompilerReferenceIdentifierCollector::default();
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression() {
                collector.visit_expression(expression);
            }
        }
        for (name, span) in collector.identifiers {
            let curated_import = self
                .imports
                .get(&name)
                .is_some_and(|binding| match binding {
                    ImportBinding::Namespace { module } => self.library.module(module).is_some(),
                    ImportBinding::Export { module, export } => {
                        self.library.module_export(module, export).is_some()
                    }
                });
            if state.env.contains_key(&name)
                || state.compiler_bindings.contains_key(&name)
                || self.signatures.contains_key(&name)
                || curated_import
                || self.library.global(&name).is_some()
            {
                continue;
            }
            let Some(hint) = self.compiler_hints.and_then(|hints| hints.get(span)) else {
                continue;
            };
            let Some(rendered) = hint.rendered_type.as_deref() else {
                continue;
            };
            let base = parse_typescript_type(rendered);
            if hint.rendered_type_is_declaration_backed
                && sort_for_base(&base) == Sort::Ref
                && !matches!(base, BaseType::Function(_, _))
                && base_may_contain_local_implementation(&base)
            {
                let _ = self.compiler_identifier_value(&name, span, state);
            }
        }
    }

    fn compiler_expression_value(
        &mut self,
        expression: &Expression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        let base = self.compiler_base(expression.span(), false)?;
        let mut validation_state = state.clone();
        let errors_before = self.errors.len();
        let valid = {
            let mut validator = CompilerSubexpressionValidator {
                verifier: self,
                state: &mut validation_state,
                valid: true,
            };
            walk_expression(&mut validator, expression);
            validator.valid
        };
        if !valid || self.errors.len() != errors_before {
            return None;
        }
        let local_implementation = {
            let mut detector = LocalImplementationDetector::new(
                &validation_state,
                self.compiler_hints,
                &self.signatures,
            );
            detector.visit_expression(expression);
            detector.found
        };
        if local_implementation && sort_for_base(&base) != Sort::Ref {
            self.error(
                "Compiler-backed scalar evidence cannot execute or depend on a locally implemented value; add an explicit refinement contract"
                    .into(),
                expression.span(),
            );
            return None;
        }
        self.join_compiler_validation_effects(state, validation_state);
        if !compiler_expression_is_pure_creation(expression) {
            self.cross_unmodeled_execution_boundary(state);
        }
        Some(self.compiler_value_from_base(
            base,
            "compiler_expression",
            local_implementation,
            state,
        ))
    }

    fn cross_unmodeled_execution_boundary(&mut self, state: &mut State) {
        state.library_semantics_intact = false;
        self.havoc_unmodeled_effects(state);
    }

    fn join_compiler_validation_effects(&mut self, state: &mut State, validated: State) {
        state.library_semantics_intact &= validated.library_semantics_intact;
        merge_reference_provenance_edges(&mut state.provenance_edges, &validated.provenance_edges);
        for reference in &validated.local_reference_provenance {
            mark_reference_aliases_as_local(state, reference);
        }
        for (name, validated_value) in &validated.env {
            if validated_value.local_implementation
                && let Some(value) = state.env.get_mut(name)
            {
                value.local_implementation = true;
            }
        }
        for (scope, validated_scope) in state.scopes.iter_mut().zip(&validated.scopes) {
            for (name, (validated_value, _)) in validated_scope {
                if validated_value
                    .as_ref()
                    .is_some_and(|value| value.local_implementation)
                    && let Some((Some(value), _)) = scope.get_mut(name)
                {
                    value.local_implementation = true;
                }
            }
        }
        for (name, validated_value) in validated.compiler_bindings {
            if let Some(value) = state.compiler_bindings.get_mut(&name) {
                value.local_implementation |= validated_value.local_implementation;
            } else {
                state.compiler_bindings.insert(name, validated_value);
            }
        }
    }

    fn havoc_immediate_callback_effects(&mut self, state: &mut State) {
        invalidate_heap_facts(state);
        let visible_names = state
            .env
            .iter()
            .filter(|(name, value)| value.mutable || state.entry_params.contains_key(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        for name in visible_names {
            let Some(value) = state.env.get_mut(&name) else {
                continue;
            };
            let term = havoc_callback_value(self, &name, value);
            if value.catalog_trusted {
                state
                    .assumptions
                    .extend(intrinsic_refinements(&value.base, &term));
            }
        }

        for scope in &mut state.scopes {
            for (name, (value, _)) in scope {
                let Some(value) = value else {
                    continue;
                };
                if !value.mutable && !state.entry_params.contains_key(name) {
                    continue;
                }
                let term = havoc_callback_value(self, name, value);
                if value.catalog_trusted {
                    state
                        .assumptions
                        .extend(intrinsic_refinements(&value.base, &term));
                }
            }
        }
    }

    /// Compiler-owned calls and getters may execute arbitrary JavaScript,
    /// including callbacks which mutate captured `let` bindings. Keep the
    /// declared base types, but sever every refinement fact that could have
    /// been invalidated by that execution boundary.
    fn havoc_unmodeled_effects(&mut self, state: &mut State) {
        invalidate_heap_facts(state);
        let visible_names = state
            .env
            .iter()
            .filter(|(name, value)| value.mutable || state.entry_params.contains_key(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        for name in visible_names {
            let Some(value) = state.env.get_mut(&name) else {
                continue;
            };
            let term = havoc_value(self, &name, value);
            if value.catalog_trusted {
                state
                    .assumptions
                    .extend(intrinsic_refinements(&value.base, &term));
            }
        }

        for scope in &mut state.scopes {
            for (name, (value, _)) in scope {
                let Some(value) = value else {
                    continue;
                };
                if !value.mutable && !state.entry_params.contains_key(name) {
                    continue;
                }
                let term = havoc_value(self, name, value);
                if value.catalog_trusted {
                    state
                        .assumptions
                        .extend(intrinsic_refinements(&value.base, &term));
                }
            }
        }
    }

    fn validate_compiler_call_arguments(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        state: &mut State,
    ) -> bool {
        let mut validation_state = state.clone();
        let errors_before = self.errors.len();
        let mut valid = true;
        for argument in &call.arguments {
            let Some(expression) = argument.as_expression() else {
                self.error(
                    "Spread arguments are outside compiler-backed refinement checking".into(),
                    argument.span(),
                );
                valid = false;
                continue;
            };
            let mut validator = CompilerSubexpressionValidator {
                verifier: self,
                state: &mut validation_state,
                valid: true,
            };
            validator.visit_expression(expression);
            valid &= validator.valid;
        }
        let valid = valid && self.errors.len() == errors_before;
        if valid {
            self.join_compiler_validation_effects(state, validation_state);
        }
        valid
    }

    fn infer_static_member(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        if member.optional {
            self.error(
                "Optional member access requires nullable type narrowing and is not yet supported"
                    .into(),
                member.span,
            );
            return None;
        }
        // `import.meta` and `new.target` do not have a useful standalone
        // refinement type, but the compiler can type their concrete member
        // expressions (for example Bun's `import.meta.path`). There are no
        // nested user expressions to bypass at this receiver boundary.
        if matches!(
            member.object,
            Expression::ImportMeta(_) | Expression::NewTarget(_)
        ) {
            let errors_before = self.errors.len();
            if let Some(value) = self.compiler_member_value(member.span, false, state) {
                return Some(value);
            }
            if self.errors.len() > errors_before {
                return None;
            }
        }
        let object = self.infer_expression(&member.object, state)?;
        if let Some(qualifier) = &object.qualifier {
            state.assumptions.push(qualifier.formula.clone());
        }
        let property = member.property.name.as_str();
        let intrinsic_property =
            object.catalog_trusted && catalog_property_is_intrinsic(&object.base, property);
        let catalog_property = object
            .catalog_trusted
            .then(|| {
                receiver_type_names(&object.base)
                    .into_iter()
                    .find_map(|receiver| {
                        self.library.receiver_property(&receiver, property).cloned()
                    })
            })
            .flatten();
        let structural_property = match (&object.base, property) {
            (BaseType::Object(fields), property) => fields
                .iter()
                .find(|(name, _)| name == property)
                .map(|(_, ty)| RefinementType::from_base(ty.clone())),
            _ => None,
        };
        let (property_refinement, property_catalog_trusted) =
            if let Some(mut property) = catalog_property {
                let trust_refinement = state.library_semantics_intact || intrinsic_property;
                if !trust_refinement {
                    property.ty.predicate = None;
                    self.havoc_unmodeled_effects(state);
                }
                let trusted =
                    trust_refinement || base_has_unambiguous_catalog_identity(&property.ty.base);
                (property.ty, trusted)
            } else if let Some(property) = structural_property {
                let catalog_trusted = base_has_unambiguous_catalog_identity(&property.base);
                (property, catalog_trusted)
            } else {
                let errors_before = self.errors.len();
                if let Some(value) =
                    self.compiler_member_value(member.span, object.local_implementation, state)
                {
                    return Some(value);
                }
                if self.errors.len() > errors_before {
                    return None;
                }
                self.error(
                    format!("No static property '{property}' on type {:?}", object.base),
                    member.span,
                );
                return None;
            };
        let property_type = property_refinement.base.clone();
        let member_sort = if property == "length" && intrinsic_property {
            Sort::Int
        } else {
            sort_for_base(&property_type)
        };
        let term = Term::Member(Box::new(object.term), property.to_string(), member_sort);
        let placeholder = Term::Var(self.fresh_name(property), term.sort());
        let mut qualifier_formula =
            Term::Same(Box::new(placeholder.clone()), Box::new(term.clone()));
        if let Some(predicate) = &property_refinement.predicate {
            let replacements = HashMap::from([("$".to_string(), term.clone())]);
            match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                Ok(formula) => {
                    state.assumptions.push(formula.clone());
                    qualifier_formula = Term::And(Box::new(qualifier_formula), Box::new(formula));
                }
                Err(message) => {
                    self.error(message, member.span);
                    return None;
                }
            }
        }
        Some(Value {
            term: term.clone(),
            base: property_type,
            declared_base: None,
            qualifier: Some(Qualifier {
                value: placeholder.clone(),
                formula: qualifier_formula,
            }),
            mutable: true,
            catalog_trusted: property_catalog_trusted,
            local_implementation: object.local_implementation,
        })
    }

    fn infer_computed_member(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        if member.optional {
            self.error(
                "Optional member access requires nullable type narrowing and is not yet supported"
                    .into(),
                member.span,
            );
            return None;
        }
        let object = self.infer_expression(&member.object, state)?;
        if let Some(qualifier) = &object.qualifier {
            state.assumptions.push(qualifier.formula.clone());
        }
        let index = self.infer_expression(&member.expression, state)?;
        if index.base != number_type() {
            let errors_before = self.errors.len();
            if let Some(value) =
                self.compiler_member_value(member.span, object.local_implementation, state)
            {
                return Some(value);
            }
            if self.errors.len() > errors_before {
                return None;
            }
            self.error(
                "Array indices must be numbers".into(),
                member.expression.span(),
            );
            return None;
        }
        let index_term = match as_int_term(&index.term) {
            Some(term) => term,
            None => {
                self.error(
                    "Array indices must be logical integers".into(),
                    member.expression.span(),
                );
                return None;
            }
        };
        let element_type = match &object.base {
            BaseType::Generic(name, arguments) if name == "DenseArray" && arguments.len() == 1 => {
                arguments[0].clone()
            }
            BaseType::Array(_) => {
                self.error(
                    "Indexed access on a possibly sparse Array may produce undefined; use a dense source or narrow the result"
                        .into(),
                    member.span,
                );
                return None;
            }
            BaseType::Primitive(name) if name == "string" => BaseType::Primitive("string".into()),
            _ => {
                let errors_before = self.errors.len();
                if let Some(value) =
                    self.compiler_member_value(member.span, object.local_implementation, state)
                {
                    return Some(value);
                }
                if self.errors.len() > errors_before {
                    return None;
                }
                self.error(
                    format!("Type {:?} does not support indexed access", object.base),
                    member.span,
                );
                return None;
            }
        };
        let length = collection_length(&object.term);
        let bounds = Term::And(
            Box::new(Term::Ge(
                Box::new(index_term.clone()),
                Box::new(Term::Int(0)),
            )),
            Box::new(Term::Lt(
                Box::new(index_term.clone()),
                Box::new(length.clone()),
            )),
        );
        if let (Some(index_value), Some(length_value)) = (
            known_number_equality(&state.assumptions, &index_term).or(match &index_term {
                Term::Int(value) => Some(*value),
                _ => None,
            }),
            known_number_equality(&state.assumptions, &length),
        ) {
            if index_value < 0 || index_value >= length_value {
                self.error(
                    "Indexed access may be outside the collection bounds".into(),
                    member.span,
                );
                return None;
            }
        } else {
            self.prove(
                &state.assumptions,
                &bounds,
                "Indexed access may be outside the collection bounds".into(),
                self.location(member.span),
            );
        }
        let term = Term::Index(
            Box::new(object.term),
            Box::new(index_term),
            sort_for_base(&element_type),
        );
        Some(Value {
            term,
            catalog_trusted: base_has_unambiguous_catalog_identity(&element_type),
            base: element_type,
            declared_base: None,
            qualifier: None,
            mutable: true,
            local_implementation: object.local_implementation,
        })
    }

    fn infer_contextual_argument(
        &mut self,
        expression: &Expression<'_>,
        expected: &RefinementType,
        state: &mut State,
        bindings: &mut HashMap<String, BaseType>,
        local_bindings: &HashMap<String, bool>,
        callback: CallbackContext<'_>,
    ) -> Option<Value> {
        if let Some(arrow) = contextual_arrow(expression) {
            return self.infer_arrow(arrow, expected, state, bindings, local_bindings, callback);
        }
        let expected = instantiate_refinement(expected, bindings);
        let value = self.infer_expression(expression, state)?;
        let compiler_checked_object_literal = matches!(expected.base, BaseType::Primitive(ref name) if name == "object")
            && contextual_object_literal(expression).is_some()
            && value.term.sort() == Sort::Ref;
        if !compiler_checked_object_literal
            && !match_base_with_bindings(&value.base, &expected.base, bindings)
        {
            self.error(
                format!(
                    "Base type mismatch: expected {:?}, found {:?}",
                    instantiate_base(&expected.base, bindings),
                    value.base
                ),
                expression.span(),
            );
        }
        Some(value)
    }

    fn infer_arrow(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'_>,
        expected: &RefinementType,
        state: &mut State,
        bindings: &mut HashMap<String, BaseType>,
        local_bindings: &HashMap<String, bool>,
        callback: CallbackContext<'_>,
    ) -> Option<Value> {
        if arrow.r#async {
            self.error(
                "Async callbacks require Promise refinement support and are not yet supported"
                    .into(),
                arrow.span,
            );
            return None;
        }
        let parameter_local_implementations = match &expected.base {
            BaseType::Function(parameters, _) => parameters
                .iter()
                .map(|parameter| {
                    type_variable_local_implementation(&parameter.ty.base, local_bindings)
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let expected = instantiate_refinement(expected, bindings);
        let BaseType::Function(expected_params, expected_return) = &expected.base else {
            self.error(
                "Arrow function passed to a non-callback parameter".into(),
                arrow.span,
            );
            return None;
        };
        if arrow.params.rest.is_some() || arrow.params.items.len() > expected_params.len() {
            self.error(
                format!(
                    "Callback accepts at most {} positional parameters",
                    expected_params.len()
                ),
                arrow.params.span,
            );
            return None;
        }

        let captured_bindings = state
            .env
            .iter()
            .chain(state.compiler_bindings.iter())
            .filter(|(_, value)| value.term.sort() == Sort::Ref)
            .map(|(name, value)| (name.clone(), value.term.clone()))
            .collect::<Vec<_>>();
        let mut callback_state = state.clone();
        // Deferred callbacks run after captured bindings may have changed.
        // Immediate callbacks still need heap/repeated-invocation facts
        // forgotten, but begin with the receiver identities evaluated for this
        // synchronous call.
        if callback.timing == Some(CallbackTiming::Immediate) {
            self.havoc_immediate_callback_effects(&mut callback_state);
        } else {
            self.havoc_unmodeled_effects(&mut callback_state);
        }
        callback_state.scopes.push(HashMap::new());
        let mut actual_params = Vec::new();
        let mut callback_parameters = HashSet::new();
        let callback_receiver_is_array = callback.receiver.is_some_and(|receiver| {
            receiver_type_names(&receiver.base)
                .iter()
                .any(|name| name == "Array")
        });
        for ((formal, expected_param), local_implementation) in arrow
            .params
            .items
            .iter()
            .zip(expected_params)
            .zip(parameter_local_implementations)
        {
            if formal.initializer.is_some() {
                self.error(
                    "Default callback parameters are outside the supported refinement subset"
                        .into(),
                    formal.span,
                );
                return None;
            }
            let BindingPattern::BindingIdentifier(identifier) = &formal.pattern else {
                self.error(
                    "Destructured callback parameters are outside the supported refinement subset"
                        .into(),
                    formal.span,
                );
                return None;
            };
            let name = identifier.name.to_string();
            callback_parameters.insert(name.clone());
            Self::initialize_name(&name, &mut callback_state);
            let parameter_sort = sort_for_base(&expected_param.ty.base);
            let term = match (
                callback.receiver.filter(|_| callback_receiver_is_array),
                expected_param.name.as_str(),
                parameter_sort,
            ) {
                (Some(receiver), "array", Sort::Ref) => receiver.term.clone(),
                (Some(receiver), "value", Sort::Ref) => Term::Index(
                    Box::new(receiver.term.clone()),
                    Box::new(Term::Var(self.fresh_name("callback_index"), Sort::Number)),
                    Sort::Ref,
                ),
                (_, "accumulator", Sort::Ref) if callback.initial_value.is_some() => {
                    callback.initial_value.expect("guarded above").term.clone()
                }
                (Some(receiver), "accumulator", Sort::Ref) => Term::Index(
                    Box::new(receiver.term.clone()),
                    Box::new(Term::Var(self.fresh_name("callback_index"), Sort::Number)),
                    Sort::Ref,
                ),
                _ => Term::Var(self.fresh_name(&name), parameter_sort),
            };
            record_reference_containment(&term, &mut callback_state.provenance_edges);
            let catalog_trusted = base_has_unambiguous_catalog_identity(&expected_param.ty.base);
            if catalog_trusted {
                callback_state
                    .assumptions
                    .extend(intrinsic_refinements(&expected_param.ty.base, &term));
            }
            callback_state.env.insert(
                name.clone(),
                Value {
                    term,
                    base: expected_param.ty.base.clone(),
                    declared_base: Some(expected_param.ty.base.clone()),
                    qualifier: None,
                    mutable: false,
                    catalog_trusted,
                    local_implementation,
                },
            );
            actual_params.push(crate::syntax::RefinedParam {
                name,
                ty: expected_param.ty.clone(),
            });
        }

        let Some(body) = arrow.get_expression() else {
            self.error(
                "Block-bodied callbacks are not yet supported by contextual refinement checking"
                    .into(),
                arrow.span,
            );
            return None;
        };
        let result = self.infer_expression(body, &mut callback_state)?;
        if !match_base_with_bindings(&result.base, &expected_return.base, bindings) {
            self.error(
                format!(
                    "Callback return type mismatch: expected {:?}, found {:?}",
                    instantiate_base(&expected_return.base, bindings),
                    result.base
                ),
                body.span(),
            );
            return None;
        }
        for (name, reference) in captured_bindings {
            let captured = if callback_parameters.contains(&name) {
                callback_state
                    .scopes
                    .last()
                    .and_then(|scope| scope.get(&name))
                    .and_then(|(value, _)| value.as_ref())
            } else {
                callback_state
                    .env
                    .get(&name)
                    .or_else(|| callback_state.compiler_bindings.get(&name))
            };
            if captured.is_some_and(|value| value.local_implementation) {
                mark_reference_aliases_as_local(state, &reference);
            }
        }
        let callback_local_references = callback_state
            .local_reference_provenance
            .iter()
            .cloned()
            .chain(
                callback_state
                    .env
                    .values()
                    .chain(callback_state.compiler_bindings.values())
                    .chain(
                        callback_state
                            .scopes
                            .iter()
                            .flat_map(|scope| scope.values())
                            .filter_map(|(value, _)| value.as_ref()),
                    )
                    .filter(|value| value.local_implementation && value.term.sort() == Sort::Ref)
                    .map(|value| value.term.clone()),
            )
            .collect::<Vec<_>>();
        merge_reference_provenance_edges(
            &mut state.provenance_edges,
            &callback_state.provenance_edges,
        );
        for reference in callback_local_references {
            mark_reference_aliases_as_local(state, &reference);
        }
        for (name, callback_value) in callback_state.compiler_bindings {
            if let Some(value) = state.compiler_bindings.get_mut(&name) {
                value.local_implementation |= callback_value.local_implementation;
                continue;
            }
            if callback_value.catalog_trusted {
                state.assumptions.extend(intrinsic_refinements(
                    &callback_value.base,
                    &callback_value.term,
                ));
            }
            state.compiler_bindings.insert(name, callback_value);
        }
        state.library_semantics_intact &= callback_state.library_semantics_intact;
        let local_implementation = result.local_implementation;
        let actual_return = RefinementType::from_base(result.base);
        Some(Value {
            term: Term::Var(self.fresh_name("callback"), Sort::Ref),
            base: BaseType::Function(actual_params, Box::new(actual_return)),
            declared_base: None,
            qualifier: None,
            mutable: true,
            catalog_trusted: true,
            local_implementation,
        })
    }

    fn library_callable(&self, expression_name: &str) -> Option<(String, Vec<FunctionSignature>)> {
        let (root, member) = expression_name
            .split_once('.')
            .map_or((expression_name, None), |(root, member)| {
                (root, Some(member))
            });
        if let Some(binding) = self.imports.get(root) {
            return match binding {
                ImportBinding::Export { module, export } if member.is_none() => {
                    let LibraryExport::Function(overloads) =
                        self.library.module_export(module, export)?
                    else {
                        return None;
                    };
                    Some((format!("{module}.{export}"), overloads.clone()))
                }
                ImportBinding::Namespace { module } => {
                    let export = member?;
                    let LibraryExport::Function(overloads) =
                        self.library.module_export(module, export)?
                    else {
                        return None;
                    };
                    Some((format!("{module}.{export}"), overloads.clone()))
                }
                _ => None,
            };
        }
        self.library
            .static_function(expression_name)
            .map(|overloads| (expression_name.to_string(), overloads.to_vec()))
    }

    fn infer_library_method(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        let receiver = self.infer_expression(&member.object, state)?;
        if let Some(qualifier) = &receiver.qualifier {
            state.assumptions.push(qualifier.formula.clone());
        }
        let method = member.property.name.as_str();
        let overloads = receiver
            .catalog_trusted
            .then(|| {
                receiver_type_names(&receiver.base)
                    .into_iter()
                    .find_map(|receiver_name| {
                        self.library
                            .receiver_method(&receiver_name, method)
                            .map(<[_]>::to_vec)
                    })
            })
            .flatten();
        let Some(overloads) = overloads else {
            let errors_before = self.errors.len();
            if let Some(value) =
                self.compiler_call_value(call, receiver.local_implementation, state)
            {
                return Some(value);
            }
            if self.errors.len() > errors_before {
                return None;
            }
            self.error(
                format!(
                    "No standard-library method '{method}' on type {:?}",
                    receiver.base
                ),
                member.span,
            );
            return None;
        };
        self.apply_library_call(
            call,
            &format!("{:?}.{method}", receiver.base),
            Some(receiver),
            &overloads,
            state,
        )
    }

    fn apply_library_call(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        display_name: &str,
        receiver: Option<Value>,
        overloads: &[FunctionSignature],
        state: &mut State,
    ) -> Option<Value> {
        let candidates: Vec<_> = overloads
            .iter()
            .filter(|signature| signature_accepts_arity(signature, call.arguments.len()))
            .cloned()
            .collect();
        if candidates.is_empty() {
            if self.compiler_hints.is_some() {
                let initial_errors = self.errors.len();
                let mut fallback_state = state.clone();
                if let Some(value) = self.compiler_call_value(
                    call,
                    receiver
                        .as_ref()
                        .is_some_and(|value| value.local_implementation),
                    &mut fallback_state,
                ) && self.errors.len() == initial_errors
                {
                    *state = fallback_state;
                    return Some(value);
                }
                self.errors.truncate(initial_errors);
            }
            let arities = overloads
                .iter()
                .map(signature_arity_description)
                .collect::<Vec<_>>()
                .join(" or ");
            self.error(
                format!(
                    "Function '{display_name}' expects {arities} arguments, got {}",
                    call.arguments.len()
                ),
                call.span,
            );
            return None;
        }

        let initial_errors = self.errors.len();
        let mut best_errors = Vec::new();
        for signature in candidates {
            self.errors.truncate(initial_errors);
            let mut candidate_state = state.clone();
            let result = self.apply_library_overload(
                call,
                display_name,
                receiver.clone(),
                &signature,
                &mut candidate_state,
            );
            if result.is_some() && self.errors.len() == initial_errors {
                *state = candidate_state;
                return result;
            }
            let candidate_errors = self.errors[initial_errors..].to_vec();
            if best_errors.is_empty() || candidate_errors.len() < best_errors.len() {
                best_errors = candidate_errors;
            }
        }
        self.errors.truncate(initial_errors);
        if self.compiler_hints.is_some() && catalog_errors_allow_compiler_fallback(&best_errors) {
            let mut fallback_state = state.clone();
            if let Some(value) = self.compiler_call_value(
                call,
                receiver
                    .as_ref()
                    .is_some_and(|value| value.local_implementation),
                &mut fallback_state,
            ) && self.errors.len() == initial_errors
            {
                *state = fallback_state;
                return Some(value);
            }
            if self.errors.len() > initial_errors {
                return None;
            }
        }
        if best_errors.is_empty() {
            self.error(
                format!("No overload of '{display_name}' accepts these arguments"),
                call.span,
            );
        } else {
            self.errors.extend(best_errors);
        }
        None
    }

    fn apply_library_overload(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        display_name: &str,
        receiver: Option<Value>,
        signature: &FunctionSignature,
        state: &mut State,
    ) -> Option<Value> {
        let trust_catalog_refinements = state.library_semantics_intact;
        let mut bindings = HashMap::new();
        let mut local_bindings = HashMap::new();
        if let (Some(receiver), Some(expected)) = (&receiver, &signature.receiver) {
            let receiver_matches =
                match_base_with_bindings(&receiver.base, &expected.base, &mut bindings)
                    || match &expected.base {
                        BaseType::Named(expected_name) => receiver_type_names(&receiver.base)
                            .iter()
                            .any(|actual_name| {
                                self.library.receiver_is_a(actual_name, expected_name)
                            }),
                        _ => false,
                    };
            if !receiver_matches {
                self.error(
                    format!(
                        "Method receiver type mismatch: expected {:?}, found {:?}",
                        expected.base, receiver.base
                    ),
                    call.callee.span(),
                );
                return None;
            }
            record_type_variable_provenance(
                &expected.base,
                receiver.local_implementation,
                &mut local_bindings,
            );
        }

        let callback_parameters = signature
            .effects
            .callbacks
            .iter()
            .map(|callback| callback.parameter_index)
            .collect::<HashSet<_>>();
        let mut deferred_callbacks = Vec::new();
        let mut arguments = vec![None; call.arguments.len()];
        for (index, argument) in call.arguments.iter().enumerate() {
            let Some(expression) = argument.as_expression() else {
                self.error(
                    "Spread arguments are not supported by refinement checking".into(),
                    argument.span(),
                );
                return None;
            };
            if callback_parameters.contains(&index) && contextual_arrow(expression).is_some() {
                deferred_callbacks.push(index);
                continue;
            }
            let parameter = signature_parameter(signature, index)?;
            let callback_timing = signature
                .effects
                .callbacks
                .iter()
                .find(|callback| callback.parameter_index == index)
                .map(|callback| callback.timing);
            let value = self.infer_contextual_argument(
                expression,
                &parameter.ty,
                state,
                &mut bindings,
                &local_bindings,
                CallbackContext {
                    timing: callback_timing,
                    receiver: receiver.as_ref(),
                    initial_value: None,
                },
            )?;
            record_library_argument_provenance(&parameter.ty.base, &value, &mut local_bindings);
            arguments[index] = Some((parameter.name.clone(), value));
        }
        for index in deferred_callbacks {
            let expression = call.arguments[index]
                .as_expression()
                .expect("spread callbacks were rejected above");
            let parameter = signature_parameter(signature, index)?;
            let callback_timing = signature
                .effects
                .callbacks
                .iter()
                .find(|callback| callback.parameter_index == index)
                .map(|callback| callback.timing);
            let callback_initial_value = arguments
                .iter()
                .flatten()
                .find(|(name, _)| name == "initialValue")
                .map(|(_, value)| value);
            let value = self.infer_contextual_argument(
                expression,
                &parameter.ty,
                state,
                &mut bindings,
                &local_bindings,
                CallbackContext {
                    timing: callback_timing,
                    receiver: receiver.as_ref(),
                    initial_value: callback_initial_value,
                },
            )?;
            record_library_argument_provenance(&parameter.ty.base, &value, &mut local_bindings);
            arguments[index] = Some((parameter.name.clone(), value));
        }
        let arguments = arguments
            .into_iter()
            .map(|argument| argument.expect("every non-spread argument was inferred"))
            .collect::<Vec<_>>();
        if signature
            .refinements
            .contains(&SemanticRefinement::ReceiverMayContainArguments)
            && let Some(receiver) = &receiver
        {
            for (_, argument) in &arguments {
                record_reference_containment_edge(
                    &argument.term,
                    &receiver.term,
                    &mut state.provenance_edges,
                );
            }
        }
        // Argument evaluation happens before the selected native function is
        // invoked and may cross an unknown execution boundary. Keep using the
        // already-resolved callable, but do not derive catalog postconditions
        // which depend on ambient state (for example Array species) afterward.
        let trust_catalog_refinements = trust_catalog_refinements && state.library_semantics_intact;
        for (index, ((_, value), argument)) in arguments.iter().zip(&call.arguments).enumerate() {
            let parameter = signature_parameter(signature, index)?;
            if library_base_requires_catalog_identity(&parameter.ty.base) && !value.catalog_trusted
            {
                self.error(
                    format!(
                        "Argument {} to '{display_name}' does not have a verified standard-library identity",
                        index + 1
                    ),
                    argument.span(),
                );
                return None;
            }
        }
        if let (Some(receiver), Some(expected)) = (&receiver, &signature.receiver) {
            record_type_variable_provenance(
                &expected.base,
                receiver.local_implementation
                    || reference_alias_has_local_implementation(state, &receiver.term),
                &mut local_bindings,
            );
        }

        // JavaScript evaluates the receiver and every argument before the
        // method body starts. Snapshot immediately before invocation so an
        // argument such as `xs.push(xs.push(2))` cannot leave us using the
        // receiver length from before the inner call.
        let old_length = receiver.as_ref().and_then(|receiver| {
            let needs_snapshot = signature.effects.receiver == ReceiverEffect::Mutate
                || !signature.effects.callbacks.is_empty()
                || signature.effects.writes_ambient_state;
            needs_snapshot.then(|| {
                let member = collection_length(&receiver.term);
                let snapshot = Term::Var(self.fresh_name("old_length"), Sort::Int);
                snapshot_heap_measure(state, &member, &snapshot);
                snapshot
            })
        });
        let invokes_unmodeled_callback = signature.effects.callbacks.iter().any(|callback| {
            call.arguments
                .get(callback.parameter_index)
                .and_then(|argument| argument.as_expression())
                .is_none_or(|expression| contextual_arrow(expression).is_none())
        });
        let has_deferred_callback = signature
            .effects
            .callbacks
            .iter()
            .any(|callback| callback.timing == CallbackTiming::Deferred);
        if !trust_catalog_refinements {
            if arguments
                .iter()
                .any(|(_, argument)| base_contains_callable_preconditions(&argument.base))
            {
                self.error(
                    "A function with refinement preconditions cannot escape through a standard-library binding after unknown code may have replaced it"
                        .into(),
                    call.span,
                );
                return None;
            }
            self.cross_unmodeled_execution_boundary(state);
        } else if signature.effects.executes_user_code || invokes_unmodeled_callback {
            self.cross_unmodeled_execution_boundary(state);
        } else if !signature.effects.callbacks.is_empty() {
            if has_deferred_callback {
                self.havoc_unmodeled_effects(state);
            } else {
                self.havoc_immediate_callback_effects(state);
            }
        }
        if invokes_unmodeled_callback {
            // A named or otherwise opaque callback has no body-level effect
            // summary. It may mutate any reference captured from this scope,
            // including the receiver and reduce's initial accumulator.
            taint_implementation_capable_references(state);
            if let (Some(receiver), Some(expected)) = (&receiver, &signature.receiver) {
                record_type_variable_provenance(
                    &expected.base,
                    base_may_contain_local_implementation(&receiver.base),
                    &mut local_bindings,
                );
            }
        }
        if let (Some(receiver), Some(expected)) = (&receiver, &signature.receiver) {
            record_type_variable_provenance(
                &expected.base,
                receiver.local_implementation
                    || reference_alias_has_local_implementation(state, &receiver.term),
                &mut local_bindings,
            );
        }
        if signature.effects.receiver == ReceiverEffect::Mutate
            && arguments
                .iter()
                .any(|(_, argument)| argument.local_implementation)
            && let Some(receiver) = &receiver
        {
            mark_reference_aliases_as_local(state, &receiver.term);
        }

        let replacements: HashMap<String, Term> = arguments
            .iter()
            .map(|(name, value)| (name.clone(), value.term.clone()))
            .chain(
                receiver
                    .iter()
                    .map(|receiver| ("this".to_string(), receiver.term.clone())),
            )
            .collect();
        if trust_catalog_refinements {
            for (index, parameter) in signature.parameters.iter().enumerate() {
                let Some(predicate) = &parameter.ty.predicate else {
                    continue;
                };
                let Some((_, _)) = arguments.get(index) else {
                    continue;
                };
                match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                    Ok(goal) => self.prove(
                        &state.assumptions,
                        &goal,
                        format!(
                            "Argument {} to '{display_name}' does not satisfy its refinement",
                            index + 1
                        ),
                        self.location(call.span),
                    ),
                    Err(message) => self.error(message, call.span),
                }
            }
        }

        let result_type = instantiate_refinement(&signature.returns, &bindings);
        let result_local_implementation =
            type_variable_local_implementation(&signature.returns.base, &local_bindings);
        let result_catalog_trusted = base_has_unambiguous_catalog_identity(&result_type.base)
            || (trust_catalog_refinements
                && library_return_has_catalog_identity(&signature.returns.base));
        let result = Term::Var(
            self.fresh_name(display_name),
            sort_for_base(&result_type.base),
        );
        let mut result_facts = if result_catalog_trusted {
            intrinsic_refinements(&result_type.base, &result)
        } else {
            Vec::new()
        };
        if trust_catalog_refinements && let Some(predicate) = &result_type.predicate {
            let mut replacements = replacements.clone();
            replacements.insert("$".into(), result.clone());
            match predicate_term(predicate, &replacements, &HashMap::new(), None) {
                Ok(formula) => result_facts.push(formula),
                Err(message) => self.error(message, call.span),
            }
        }
        if trust_catalog_refinements && let Some(receiver) = &receiver {
            let receiver_length = old_length.unwrap_or_else(|| collection_length(&receiver.term));
            let result_length = collection_length(&result);
            for refinement in &signature.refinements {
                match refinement {
                    SemanticRefinement::ResultLengthEqualsReceiver => {
                        result_facts.push(Term::Same(
                            Box::new(result_length.clone()),
                            Box::new(receiver_length.clone()),
                        ))
                    }
                    SemanticRefinement::ResultLengthAtMostReceiver => result_facts.push(Term::Le(
                        Box::new(result_length.clone()),
                        Box::new(receiver_length.clone()),
                    )),
                    SemanticRefinement::ReceiverLengthIncreasesByArgumentCount => {
                        let post_length = Term::Add(
                            Box::new(receiver_length.clone()),
                            Box::new(Term::Int(call.arguments.len() as i64)),
                        );
                        result_facts.push(Term::Same(
                            Box::new(result.clone()),
                            Box::new(post_length.clone()),
                        ));
                        result_facts.push(Term::Same(
                            Box::new(collection_length(&receiver.term)),
                            Box::new(post_length),
                        ));
                        for (offset, (_, argument)) in arguments.iter().enumerate() {
                            let index = Term::Add(
                                Box::new(receiver_length.clone()),
                                Box::new(Term::Int(offset as i64)),
                            );
                            result_facts.push(Term::Same(
                                Box::new(Term::Index(
                                    Box::new(receiver.term.clone()),
                                    Box::new(index),
                                    argument.term.sort(),
                                )),
                                Box::new(argument.term.clone()),
                            ));
                        }
                    }
                    SemanticRefinement::RequiresPositiveReceiverLength => {
                        self.prove(
                            &state.assumptions,
                            &Term::Gt(Box::new(receiver_length.clone()), Box::new(Term::Int(0))),
                            format!("'{display_name}' requires a non-empty dense array"),
                            self.location(call.span),
                        );
                    }
                    SemanticRefinement::ReceiverLengthDecreasesByOne => {
                        let last =
                            Term::Sub(Box::new(receiver_length.clone()), Box::new(Term::Int(1)));
                        result_facts.push(Term::Same(
                            Box::new(result.clone()),
                            Box::new(Term::Index(
                                Box::new(receiver.term.clone()),
                                Box::new(last.clone()),
                                result.sort(),
                            )),
                        ));
                        result_facts.push(Term::Same(
                            Box::new(collection_length(&receiver.term)),
                            Box::new(last),
                        ));
                    }
                    SemanticRefinement::TypeGuard { .. }
                    | SemanticRefinement::ResultElementsFromCallback { .. }
                    | SemanticRefinement::ResultElementsSubsetOfReceiver
                    | SemanticRefinement::ReceiverMayContainArguments => {}
                }
            }
        }
        state.assumptions.extend(result_facts.iter().cloned());
        let qualifier = (!result_facts.is_empty()).then(|| Qualifier {
            value: result.clone(),
            formula: and_terms(result_facts),
        });
        Some(Value {
            term: result,
            base: result_type.base,
            declared_base: None,
            qualifier,
            mutable: true,
            catalog_trusted: result_catalog_trusted,
            local_implementation: result_local_implementation,
        })
    }

    fn infer_call(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        state: &mut State,
    ) -> Option<Value> {
        let root_name = expression_root_name(&call.callee);
        if let Some(root_name) = &root_name
            && state.uninitialized.contains(root_name)
        {
            self.error(
                format!("Call target '{root_name}' is used before its declaration"),
                call.callee.span(),
            );
            return None;
        }
        let invokes_named_contract_directly = match &call.callee {
            Expression::Identifier(identifier) => {
                !state.env.contains_key(identifier.name.as_str())
                    && self.signatures.contains_key(identifier.name.as_str())
            }
            _ => false,
        };
        if !invokes_named_contract_directly
            && self
                .known_expression_base(&call.callee, state)
                .is_some_and(|base| base_contains_callable_preconditions(&base))
        {
            let display_name =
                expression_name(&call.callee).unwrap_or_else(|| "function value".to_string());
            self.error(
                format!(
                    "Calling refined function value '{display_name}' through an alias or member is outside the supported refinement subset"
                ),
                call.callee.span(),
            );
            return None;
        }
        let Some(function_name) = expression_name(&call.callee) else {
            if let Expression::StaticMemberExpression(member) = &call.callee {
                return self.infer_library_method(call, member, state);
            }
            let errors_before = self.errors.len();
            if let Some(value) = self.compiler_call_value(call, false, state) {
                return Some(value);
            }
            if self.errors.len() > errors_before {
                return None;
            }
            self.error(
                "Unsupported call target in refinement analysis".into(),
                call.callee.span(),
            );
            return None;
        };

        let root_is_lexical = root_name
            .as_ref()
            .is_some_and(|root| state.env.contains_key(root));
        let root_is_import = !root_is_lexical
            && root_name
                .as_ref()
                .is_some_and(|root| self.imports.contains_key(root));
        let root_is_top_level_function = root_name
            .as_ref()
            .is_some_and(|root| self.top_level_bindings.contains(root));
        let root_is_local = root_name.as_ref().is_some_and(|root| {
            state.env.contains_key(root)
                || self.imports.contains_key(root)
                || self.top_level_bindings.contains(root)
        });
        if (!root_is_local || root_is_import)
            && let Some((display_name, overloads)) = self.library_callable(&function_name)
        {
            return self.apply_library_call(call, &display_name, None, &overloads, state);
        }
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let errors_before_method = self.errors.len();
            if let Some(value) = self.infer_library_method(call, member, state) {
                return Some(value);
            }
            if self.errors.len() > errors_before_method {
                return None;
            }
            if root_is_local {
                return None;
            }
        }
        if root_is_lexical {
            self.error(
                format!(
                    "Local function value '{function_name}' requires an explicit refinement contract before its return type can be used in refinement checking"
                ),
                call.callee.span(),
            );
            return None;
        }
        let Some(contract) = self.signatures.get(&function_name).cloned() else {
            if root_is_top_level_function {
                self.error(
                    format!(
                        "Local function '{function_name}' requires an explicit refinement contract before its return type can be used in refinement checking"
                    ),
                    call.callee.span(),
                );
                return None;
            }
            let errors_before = self.errors.len();
            if let Some(value) = self.compiler_call_value(call, false, state) {
                return Some(value);
            }
            if self.errors.len() > errors_before {
                return None;
            }
            if root_is_local {
                self.error(
                    format!("Call target '{function_name}' is shadowed by a local binding"),
                    call.callee.span(),
                );
                return None;
            }
            self.error(
                format!("No refinement signature for function '{function_name}'"),
                call.span,
            );
            return None;
        };
        if call.arguments.len() != contract.params.len() {
            self.error(
                format!(
                    "Function '{function_name}' expects {} arguments, got {}",
                    contract.params.len(),
                    call.arguments.len()
                ),
                call.span,
            );
            return None;
        }
        let mut arguments = Vec::new();
        for argument in &call.arguments {
            let Some(expression) = argument.as_expression() else {
                self.error(
                    "Spread arguments are not supported by refinement checking".into(),
                    argument.span(),
                );
                return None;
            };
            arguments.push(self.infer_expression(expression, state)?);
        }

        for argument in &arguments {
            if let Some(qualifier) = &argument.qualifier {
                state.assumptions.push(qualifier.formula.clone());
            }
        }
        let replacements: HashMap<String, Term> = contract
            .params
            .iter()
            .zip(&arguments)
            .map(|((name, _), value)| (name.clone(), value.term.clone()))
            .collect();
        let mut predicate_arguments = HashMap::new();
        for predicate_name in &contract.predicate_params {
            for ((_, param_type), argument) in contract.params.iter().zip(&arguments) {
                if contains_predicate(param_type.predicate.as_ref(), predicate_name)
                    && let Some(qualifier) = &argument.qualifier
                {
                    predicate_arguments.insert(predicate_name.clone(), qualifier.clone());
                    break;
                }
            }
            if !predicate_arguments.contains_key(predicate_name) {
                self.error(
                    format!("Cannot infer refinement predicate '{predicate_name}' at call to '{function_name}'"),
                    call.span,
                );
                return None;
            }
        }

        let index_names = index_names_in_contract(&contract);
        for (((name, parameter), argument), arg_index) in
            contract.params.iter().zip(&arguments).zip(0usize..)
        {
            if index_names.contains(name)
                && matches!(&parameter.base, BaseType::Primitive(kind) if kind == "number")
                && as_int_term(&argument.term).is_none()
            {
                self.error(
                    format!(
                        "Index parameter '{name}' of '{function_name}' requires a safe integer"
                    ),
                    call.arguments[arg_index].span(),
                );
            }
            self.check_base(&argument.base, &parameter.base, call.span);
            if base_requires_catalog_identity(&parameter.base) && !argument.catalog_trusted {
                self.error(
                    format!(
                        "Argument {} to '{function_name}' does not have a verified standard-library identity",
                        arg_index + 1
                    ),
                    call.arguments[arg_index].span(),
                );
            }
            if let Some(index) = &parameter.index {
                self.prove_index(
                    argument,
                    index,
                    &replacements,
                    &state.assumptions,
                    format!(
                        "Argument {} to '{function_name}' does not match its index",
                        arg_index + 1
                    ),
                    self.location(call.span),
                    call.span,
                );
            }
            if let Some(predicate) = &parameter.predicate {
                match predicate_term(predicate, &replacements, &predicate_arguments, None) {
                    Ok(goal) => self.prove(
                        &state.assumptions,
                        &goal,
                        format!(
                            "Argument {} to '{function_name}' does not satisfy its refinement",
                            arg_index + 1
                        ),
                        self.location(call.span),
                    ),
                    Err(message) => self.error(message, call.span),
                }
            }
        }

        // User contracts do not yet carry an effect summary. Besides mutating
        // object arguments, compiler-backed code in the function may reach a
        // captured mutable binding, so conservatively forget both kinds of
        // caller refinements at every user-function boundary.
        let escaped_references = arguments
            .iter()
            .filter(|argument| {
                argument.term.sort() == Sort::Ref
                    && base_may_contain_local_implementation(&argument.base)
            })
            .map(|argument| argument.term.clone())
            .collect::<Vec<_>>();
        self.cross_unmodeled_execution_boundary(state);
        taint_implementation_capable_references(state);
        for reference in escaped_references {
            mark_reference_aliases_as_local(state, &reference);
        }

        let result = Term::Var(
            self.fresh_name(&function_name),
            sort_for_indexed_type(&contract.ret),
        );
        let mut result_facts = Vec::new();
        if let Some(index) = &contract.ret.index {
            match predicate_term(index, &replacements, &predicate_arguments, Some(Sort::Int)) {
                Ok(index_term) => {
                    let result_value = Value {
                        term: result.clone(),
                        base: contract.ret.base.clone(),
                        declared_base: None,
                        qualifier: None,
                        mutable: true,
                        catalog_trusted: declared_base_has_catalog_identity(&contract.ret.base),
                        local_implementation: false,
                    };
                    result_facts.push(index_formula(&result_value, &index_term));
                }
                Err(message) => self.error(message, call.span),
            }
        }
        if let Some(predicate) = &contract.ret.predicate {
            let mut result_replacements = replacements;
            result_replacements.insert("$".into(), result.clone());
            match predicate_term(
                predicate,
                &result_replacements,
                &predicate_arguments,
                Some(result.sort()),
            ) {
                Ok(formula) => result_facts.push(formula),
                Err(message) => self.error(message, call.span),
            }
        }
        state.assumptions.extend(result_facts.iter().cloned());
        let qualifier = (!result_facts.is_empty()).then(|| Qualifier {
            value: result.clone(),
            formula: and_terms(result_facts),
        });
        state
            .assumptions
            .extend(intrinsic_refinements(&contract.ret.base, &result));
        let local_implementation = base_may_contain_local_implementation(&contract.ret.base);
        Some(Value {
            term: result,
            catalog_trusted: declared_base_has_catalog_identity(&contract.ret.base),
            base: contract.ret.base,
            declared_base: None,
            qualifier,
            mutable: true,
            local_implementation,
        })
    }

    fn check_base(&mut self, actual: &BaseType, expected: &BaseType, span: Span) {
        if !bases_compatible(actual, expected) {
            self.error(
                format!(
                    "Base type mismatch: expected {:?}, found {:?}",
                    expected, actual
                ),
                span,
            );
        }
    }

    fn bind_value(&mut self, name: &str, value: Value, state: &mut State) {
        let symbol = Term::Var(self.fresh_name(name), value.term.sort());
        let local_reference = value.local_implementation && symbol.sort() == Sort::Ref;
        let binding_fact = Term::Same(Box::new(symbol.clone()), Box::new(value.term.clone()));
        record_reference_provenance_edges(&binding_fact, &mut state.provenance_edges);
        state.assumptions.push(binding_fact);
        if value.catalog_trusted {
            state
                .assumptions
                .extend(intrinsic_refinements(&value.base, &symbol));
        }

        let qualifier = value.qualifier.map(|qualifier| {
            let formula = substitute(&qualifier.formula, &qualifier.value, &symbol);
            record_reference_provenance_edges(&formula, &mut state.provenance_edges);
            state.assumptions.push(formula.clone());
            Qualifier {
                value: symbol.clone(),
                formula,
            }
        });
        state.env.insert(
            name.to_string(),
            Value {
                term: symbol.clone(),
                base: value.base,
                declared_base: value.declared_base,
                qualifier,
                mutable: value.mutable,
                catalog_trusted: value.catalog_trusted,
                local_implementation: value.local_implementation,
            },
        );
        if local_reference {
            mark_reference_aliases_as_local(state, &symbol);
        }
    }

    fn enter_scope(statements: &[&Statement<'_>], state: &mut State) {
        state.scopes.push(HashMap::new());
        for statement in statements {
            let Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            if declaration.kind.is_var() || declaration.kind.is_using() {
                continue;
            }
            for declarator in &declaration.declarations {
                let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                    continue;
                };
                let name = identifier.name.to_string();
                if state.scopes.last().unwrap().contains_key(&name) {
                    continue;
                }
                let previous = state.env.remove(&name);
                let previously_uninitialized = state.uninitialized.remove(&name);
                state
                    .scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), (previous, previously_uninitialized));
                state.uninitialized.insert(name);
            }
        }
    }

    fn initialize_name(name: &str, state: &mut State) -> bool {
        if state.scopes.is_empty() {
            state.scopes.push(HashMap::new());
        }
        if state.scopes.last().unwrap().contains_key(name) {
            return state.uninitialized.remove(name);
        }
        let previous = state.env.remove(name);
        let previously_uninitialized = state.uninitialized.remove(name);
        state
            .scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (previous, previously_uninitialized));
        true
    }

    fn leave_scope(state: &mut State) {
        let Some(scope) = state.scopes.pop() else {
            return;
        };
        for (name, (previous, previously_uninitialized)) in scope {
            state.env.remove(&name);
            if let Some(previous) = previous {
                state.env.insert(name.clone(), previous);
            }
            if previously_uninitialized {
                state.uninitialized.insert(name);
            } else {
                state.uninitialized.remove(&name);
            }
        }
    }

    fn narrow_qualifiers(state: &mut State, fact: &Term) {
        for value in state.env.values_mut() {
            if !contains_term(fact, &value.term) {
                continue;
            }
            let formula = match value.qualifier.take() {
                Some(previous) => Term::And(Box::new(previous.formula), Box::new(fact.clone())),
                None => fact.clone(),
            };
            value.qualifier = Some(Qualifier {
                value: value.term.clone(),
                formula,
            });
        }
    }

    fn path_is_reachable(state: &State) -> bool {
        let impossible = Term::Bool(false);
        let constraint = FixpointConstraint {
            assumptions: &state.assumptions,
            consequent: &impossible,
        };
        !matches!(solve_constraint(&constraint), Ok(SatResult::Unsat))
    }

    fn prove(&mut self, assumptions: &[Term], goal: &Term, message: String, loc: SourceLocation) {
        let constraint = FixpointConstraint {
            assumptions,
            consequent: goal,
        };
        match solve_constraint(&constraint) {
            Ok(SatResult::Unsat) => {}
            Ok(SatResult::Sat) => self.errors.push(RtError {
                message,
                loc: Some(loc),
            }),
            Ok(SatResult::Unknown) => self.errors.push(RtError {
                message: format!("Z3 returned unknown while checking: {message}"),
                loc: Some(loc),
            }),
            Err(error) => self.errors.push(RtError {
                message: error,
                loc: Some(loc),
            }),
        }
    }

    fn obligation_holds(&self, assumptions: &[Term], goal: &Term) -> bool {
        let constraint = FixpointConstraint {
            assumptions,
            consequent: goal,
        };
        matches!(solve_constraint(&constraint), Ok(SatResult::Unsat))
    }

    #[allow(clippy::too_many_arguments)]
    fn prove_index(
        &mut self,
        value: &Value,
        index: &PredicateExpr,
        replacements: &HashMap<String, Term>,
        assumptions: &[Term],
        message: String,
        loc: SourceLocation,
        span: Span,
    ) {
        match predicate_term(index, replacements, &HashMap::new(), Some(Sort::Int)) {
            Ok(index_term) => {
                let goal = index_formula(value, &index_term);
                self.prove(assumptions, &goal, message, loc);
            }
            Err(error) => self.error(error, span),
        }
    }

    fn verify_loop(
        &mut self,
        test: Option<&Expression<'_>>,
        update: Option<&Expression<'_>>,
        body: &Statement<'_>,
        span: Span,
        states: Vec<State>,
        current_function: Option<(&str, &Contract)>,
    ) -> Vec<State> {
        if states.is_empty() {
            return Vec::new();
        }
        let assigned = assigned_names_in_statement(body)
            .union(&update.map(assigned_names_in_expression).unwrap_or_default())
            .cloned()
            .collect::<HashSet<_>>();
        let mut invariants = scrape_loop_candidates(&states, &assigned, current_function);
        loop {
            let current = invariants.clone();
            let mut next = Vec::new();
            for candidate in &current {
                if self.candidate_holds_on_entry(&states, candidate)
                    && self.candidate_preserved_by_body(
                        &states,
                        candidate,
                        &current,
                        test,
                        update,
                        body,
                        current_function,
                        span,
                    )
                {
                    next.push(candidate.clone());
                }
            }
            if next.len() == current.len() {
                invariants = next;
                break;
            }
            invariants = next;
        }

        let mut output = Vec::new();
        for state in states {
            let mut body_state = state.clone();
            self.havoc_assigned(&assigned, &mut body_state);
            let extra = invariants
                .iter()
                .filter_map(|candidate| candidate.instantiate(&body_state))
                .collect::<Vec<_>>();
            body_state.assumptions.extend(extra.clone());
            if let Some(test) = test {
                let Some(test_value) = self.infer_expression(test, &mut body_state) else {
                    continue;
                };
                if value_sort(&test_value) != Some(Sort::Bool)
                    && test_value.term.sort() != Sort::Bool
                {
                    self.error("Loop condition must be boolean".into(), test.span());
                    continue;
                }
                let mut taken = body_state.clone();
                taken.assumptions.push(test_value.term.clone());
                let mut after_body = self.verify_statement(body, vec![taken], current_function);
                if let Some(update) = update {
                    for state in &mut after_body {
                        self.infer_expression(update, state);
                    }
                }
                let mut exit = body_state;
                exit.assumptions.push(Term::Not(Box::new(test_value.term)));
                output.push(exit);
            } else {
                let _ = self.verify_statement(body, vec![body_state.clone()], current_function);
                output.push(body_state);
            }
        }
        output
    }

    fn candidate_holds_on_entry(&self, states: &[State], candidate: &LoopCandidate) -> bool {
        states.iter().all(|state| {
            candidate
                .instantiate(state)
                .is_some_and(|goal| self.obligation_holds(&state.assumptions, &goal))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn candidate_preserved_by_body(
        &mut self,
        states: &[State],
        candidate: &LoopCandidate,
        invariants: &[LoopCandidate],
        test: Option<&Expression<'_>>,
        update: Option<&Expression<'_>>,
        body: &Statement<'_>,
        current_function: Option<(&str, &Contract)>,
        span: Span,
    ) -> bool {
        let errors_before = self.errors.len();
        let assigned = assigned_names_in_statement(body)
            .union(&update.map(assigned_names_in_expression).unwrap_or_default())
            .cloned()
            .collect::<HashSet<_>>();
        for state in states {
            let mut head = state.clone();
            self.havoc_assigned(&assigned, &mut head);
            let extra = invariants
                .iter()
                .filter_map(|candidate| candidate.instantiate(&head))
                .collect::<Vec<_>>();
            head.assumptions.extend(extra);
            if let Some(test) = test {
                let Some(test_value) = self.infer_expression(test, &mut head) else {
                    self.errors.truncate(errors_before);
                    return false;
                };
                head.assumptions.push(test_value.term);
            }
            let mut after = self.verify_statement(body, vec![head], current_function);
            if let Some(update) = update {
                for state in &mut after {
                    self.infer_expression(update, state);
                }
            }
            let preserved = !after.is_empty()
                && after.iter().all(|state| {
                    candidate
                        .instantiate(state)
                        .is_some_and(|goal| self.obligation_holds(&state.assumptions, &goal))
                });
            self.errors.truncate(errors_before);
            if !preserved {
                let _ = span;
                return false;
            }
        }
        true
    }

    fn havoc_assigned(&mut self, assigned: &HashSet<String>, state: &mut State) {
        for name in assigned {
            let Some(previous) = state.env.get(name).cloned() else {
                continue;
            };
            let fresh = Term::Var(self.fresh_name(name), previous.term.sort());
            if let Some(qualifier) = &previous.qualifier {
                state
                    .assumptions
                    .retain(|fact| !contains_term(fact, &previous.term));
                let _ = qualifier;
            }
            state
                .assumptions
                .retain(|fact| !contains_term(fact, &previous.term));
            let mut next = previous;
            next.term = fresh;
            next.qualifier = None;
            state.env.insert(name.clone(), next);
        }
        // The loop exit is this havoc'd head plus the negated test, not the
        // body's post-state. Keeping the entry length would accept empty pop
        // and OOB indexing after a loop that drained the array.
        invalidate_heap_facts(state);
    }

    fn fresh_name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}#{}", self.fresh)
    }

    fn error(&mut self, message: String, span: Span) {
        self.errors.push(RtError {
            message,
            loc: Some(self.location(span)),
        });
    }

    fn location(&self, span: Span) -> SourceLocation {
        let offset = span.start as usize;
        let prefix = &self.source[..offset.min(self.source.len())];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column = prefix
            .rsplit_once('\n')
            .map_or(prefix.len(), |(_, tail)| tail.len()) as u32
            + 1;
        SourceLocation {
            file: Some(self.file_name.into()),
            line,
            column,
        }
    }
}

fn int_conversion_axioms(terms: &[&Term]) -> Vec<Term> {
    let mut literals = HashSet::new();
    for term in terms {
        collect_int_literals(term, &mut literals);
    }
    literals
        .into_iter()
        .map(|value| {
            Term::Eq(
                Box::new(Term::ToNumber(Box::new(Term::Int(value)))),
                Box::new(Term::Number(value)),
            )
        })
        .collect()
}

fn collect_int_literals(term: &Term, output: &mut HashSet<i64>) {
    match term {
        Term::Int(value) => {
            output.insert(*value);
        }
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right)
        | Term::Index(left, right, _) => {
            collect_int_literals(left, output);
            collect_int_literals(right, output);
        }
        Term::Not(inner)
        | Term::Pred(_, inner)
        | Term::Member(inner, _, _)
        | Term::ToNumber(inner) => collect_int_literals(inner, output),
        Term::Number(_) | Term::Bool(_) | Term::String(_) | Term::Var(_, _) => {}
    }
}

fn solve_constraint(constraint: &FixpointConstraint<'_>) -> Result<SatResult, String> {
    let (mut assumptions, consequent) = abstract_predicate_applications(constraint)?;
    assumptions.extend(int_conversion_axioms(
        &assumptions
            .iter()
            .chain(std::iter::once(&consequent))
            .collect::<Vec<_>>(),
    ));
    if assumptions.contains(&consequent) {
        return Ok(SatResult::Unsat);
    }
    let constraint = FixpointConstraint {
        assumptions: &assumptions,
        consequent: &consequent,
    };
    let fixedpoint = Fixedpoint::new();
    let bool_sort = Z3Sort::bool();
    let bad_decl = FuncDecl::new("__rt_bad", &[], &bool_sort);
    fixedpoint.register_relation(&bad_decl);
    let bad = bad_decl.apply(&[]).as_bool().unwrap();

    let mut body = Vec::new();
    for assumption in &assumptions {
        match to_z3(assumption)? {
            ZTerm::Bool(value) => body.push(value),
            ZTerm::Number(_) | ZTerm::Int(_) | ZTerm::String(_) | ZTerm::Ref(_) => {
                return Err("Fixpoint assumption is not boolean".into());
            }
        }
    }
    let ZTerm::Bool(goal) = to_z3(&consequent)? else {
        return Err("Refinement predicate is not boolean".into());
    };
    body.push(goal.not());
    let body_refs: Vec<&Bool> = body.iter().collect();
    let rule = Bool::and(&body_refs).implies(&bad);

    let variables = constraint_variables(&constraint);
    let number_vars: Vec<Float> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Number)
        .map(|(name, _)| Float::new_const_double(name.as_str()))
        .collect();
    let int_vars: Vec<Z3Int> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Int)
        .map(|(name, _)| Z3Int::new_const(name.as_str()))
        .collect();
    let bool_vars: Vec<Bool> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Bool)
        .map(|(name, _)| Bool::new_const(name.as_str()))
        .collect();
    let string_vars: Vec<Z3String> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::String)
        .map(|(name, _)| Z3String::new_const(name.as_str()))
        .collect();
    let reference_sort = Z3Sort::uninterpreted(Symbol::String("Ref".into()));
    let reference_vars: Vec<Dynamic> = variables
        .iter()
        .filter(|(_, sort)| *sort == Sort::Ref)
        .map(|(name, _)| Dynamic::new_const(name.as_str(), &reference_sort))
        .collect();
    let mut bounds: Vec<&dyn Ast> = number_vars.iter().map(|var| var as &dyn Ast).collect();
    bounds.extend(int_vars.iter().map(|var| var as &dyn Ast));
    bounds.extend(bool_vars.iter().map(|var| var as &dyn Ast));
    bounds.extend(string_vars.iter().map(|var| var as &dyn Ast));
    bounds.extend(reference_vars.iter().map(|var| var as &dyn Ast));
    let rule = ast::forall_const(&bounds, &[], &rule);
    fixedpoint.add_rule(&rule, Some("refinement_violation"));
    match fixedpoint.query(&bad) {
        SatResult::Sat => Ok(SatResult::Sat),
        SatResult::Unknown | SatResult::Unsat => {
            // Fixedpoint may report Unsat for IEEE-754 obligations that SMT
            // still finds a counterexample for (for example `x === x` on NaN).
            // Only SMT Unsat is a proof; Sat/Unknown fail closed.
            solve_constraint_with_smt(&assumptions, &consequent)
        }
    }
}

/// Fixedpoint can return `unknown` for otherwise decidable formulas combining
/// floating-point terms with uninterpreted member functions. The same liquid
/// implication is a quantifier-free SMT query when its free variables are
/// represented as constants, so use the general solver as a sound fallback.
fn solve_constraint_with_smt(assumptions: &[Term], consequent: &Term) -> Result<SatResult, String> {
    let solver = Solver::new();
    for assumption in assumptions {
        let ZTerm::Bool(assumption) = to_z3(assumption)? else {
            return Err("SMT assumption is not boolean".into());
        };
        solver.assert(&assumption);
    }
    let ZTerm::Bool(goal) = to_z3(consequent)? else {
        return Err("Refinement predicate is not boolean".into());
    };
    solver.assert(goal.not());
    Ok(solver.check())
}

/// Fixedpoint relations have least-model semantics. Registering an abstract
/// predicate parameter such as `p` without defining rules would therefore
/// make `p` empty and could prove arbitrary consequences from `p(x)`.
///
/// Treat every predicate application as a freely chosen Boolean atom instead.
/// Pairwise congruence constraints preserve the uninterpreted-function law
/// that equal arguments have equal predicate results. The resulting formula
/// is still checked as a Horn rule by `Fixedpoint`, but no empty least model
/// can discharge a polymorphic obligation vacuously.
fn abstract_predicate_applications(
    constraint: &FixpointConstraint<'_>,
) -> Result<(Vec<Term>, Term), String> {
    let mut atoms = Vec::new();
    let mut assumptions: Vec<Term> = constraint
        .assumptions
        .iter()
        .map(|term| replace_predicate_applications(term, &mut atoms))
        .collect();
    let consequent = replace_predicate_applications(constraint.consequent, &mut atoms);

    let mut domains = HashMap::new();
    for (name, argument, _) in &atoms {
        let sort = argument.sort();
        if domains
            .insert(name.clone(), sort)
            .is_some_and(|found| found != sort)
        {
            return Err(format!(
                "Predicate parameter '{name}' is applied to incompatible base types"
            ));
        }
    }

    for left_index in 0..atoms.len() {
        for right_index in (left_index + 1)..atoms.len() {
            let (left_name, left_argument, left_atom) = &atoms[left_index];
            let (right_name, right_argument, right_atom) = &atoms[right_index];
            if left_name != right_name {
                continue;
            }
            let arguments_differ = Term::Not(Box::new(Term::Same(
                Box::new(left_argument.clone()),
                Box::new(right_argument.clone()),
            )));
            let results_match = Term::Eq(Box::new(left_atom.clone()), Box::new(right_atom.clone()));
            assumptions.push(Term::Or(
                Box::new(arguments_differ),
                Box::new(results_match),
            ));
        }
    }

    Ok((assumptions, consequent))
}

fn replace_predicate_applications(term: &Term, atoms: &mut Vec<(String, Term, Term)>) -> Term {
    match term {
        Term::Pred(name, argument) => {
            let argument = replace_predicate_applications(argument, atoms);
            if let Some((_, _, atom)) = atoms.iter().find(|(found_name, found_argument, _)| {
                found_name == name && found_argument == &argument
            }) {
                return atom.clone();
            }
            let atom = Term::Var(format!("@predicate_atom:{}", atoms.len()), Sort::Bool);
            atoms.push((name.clone(), argument, atom.clone()));
            atom
        }
        Term::Member(object, property, sort) => Term::Member(
            Box::new(replace_predicate_applications(object, atoms)),
            property.clone(),
            *sort,
        ),
        Term::Index(object, index, sort) => Term::Index(
            Box::new(replace_predicate_applications(object, atoms)),
            Box::new(replace_predicate_applications(index, atoms)),
            *sort,
        ),
        Term::Add(left, right) => Term::Add(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Sub(left, right) => Term::Sub(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Mul(left, right) => Term::Mul(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Same(left, right) => Term::Same(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Eq(left, right) => Term::Eq(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Ne(left, right) => Term::Ne(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Gt(left, right) => Term::Gt(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Lt(left, right) => Term::Lt(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Ge(left, right) => Term::Ge(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Le(left, right) => Term::Le(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::And(left, right) => Term::And(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Or(left, right) => Term::Or(
            Box::new(replace_predicate_applications(left, atoms)),
            Box::new(replace_predicate_applications(right, atoms)),
        ),
        Term::Not(inner) => Term::Not(Box::new(replace_predicate_applications(inner, atoms))),
        Term::ToNumber(inner) => {
            Term::ToNumber(Box::new(replace_predicate_applications(inner, atoms)))
        }
        Term::Number(_) | Term::Int(_) | Term::Bool(_) | Term::String(_) | Term::Var(_, _) => {
            term.clone()
        }
    }
}

fn predicate_term(
    predicate: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    predicate_arguments: &HashMap<String, Qualifier>,
    expected: Option<Sort>,
) -> Result<Term, String> {
    match predicate {
        PredicateExpr::Literal(Literal::Number(value))
            if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991_f64 =>
        {
            if expected == Some(Sort::Number) {
                Ok(Term::Number(*value as i64))
            } else {
                Ok(Term::Int(*value as i64))
            }
        }
        PredicateExpr::Literal(Literal::Boolean(value)) => Ok(Term::Bool(*value)),
        PredicateExpr::Literal(Literal::String(value)) => Ok(Term::String(value.clone())),
        PredicateExpr::Literal(Literal::Number(_)) => {
            Err("Only safe integer-valued number refinements are supported".into())
        }
        PredicateExpr::Identifier(name) => replacements
            .get(name)
            .cloned()
            .ok_or_else(|| format!("No symbolic value for '{name}'")),
        PredicateExpr::Return => replacements
            .get("$")
            .cloned()
            .ok_or_else(|| "No symbolic return value".into()),
        PredicateExpr::Member(object, property) => {
            let object = predicate_term(object, replacements, predicate_arguments, None)?;
            let sort = if property == "length" {
                Sort::Int
            } else {
                expected.unwrap_or(Sort::Number)
            };
            Ok(Term::Member(Box::new(object), property.clone(), sort))
        }
        PredicateExpr::PredicateApply(name, argument) => {
            let argument = predicate_term(argument, replacements, predicate_arguments, None)?;
            if let Some(qualifier) = predicate_arguments.get(name) {
                Ok(substitute(&qualifier.formula, &qualifier.value, &argument))
            } else {
                Ok(Term::Pred(name.clone(), Box::new(argument)))
            }
        }
        PredicateExpr::Not(inner) => {
            let inner = predicate_term(inner, replacements, predicate_arguments, Some(Sort::Bool))?;
            if inner.sort() != Sort::Bool {
                return Err("Logical not requires a boolean predicate".into());
            }
            Ok(Term::Not(Box::new(inner)))
        }
        PredicateExpr::Logical(operator, left, right) => {
            let left = predicate_term(left, replacements, predicate_arguments, Some(Sort::Bool))?;
            let right = predicate_term(right, replacements, predicate_arguments, Some(Sort::Bool))?;
            if left.sort() != Sort::Bool || right.sort() != Sort::Bool {
                return Err("Logical predicates require boolean operands".into());
            }
            Ok(match operator {
                LogicalOp::And => Term::And(Box::new(left), Box::new(right)),
                LogicalOp::Or => Term::Or(Box::new(left), Box::new(right)),
            })
        }
        PredicateExpr::Binary(operator, left, right) => {
            let operand_sort = match operator {
                BinaryOp::EqEqEq | BinaryOp::NotEqEq | BinaryOp::EqEq | BinaryOp::NotEq => {
                    predicate_literal_sort(left)
                        .or_else(|| predicate_literal_sort(right))
                        .unwrap_or_else(|| {
                            inferred_numeric_sort(left, right, replacements, expected)
                        })
                }
                _ => inferred_numeric_sort(left, right, replacements, expected),
            };
            let left = predicate_term(left, replacements, predicate_arguments, Some(operand_sort))?;
            let right =
                predicate_term(right, replacements, predicate_arguments, Some(operand_sort))?;
            predicate_binary(operator, left, right)
        }
    }
}

fn inferred_numeric_sort(
    left: &PredicateExpr,
    right: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    expected: Option<Sort>,
) -> Sort {
    if expected == Some(Sort::Int) {
        return Sort::Int;
    }
    if expected == Some(Sort::Number) {
        return Sort::Number;
    }
    let mut sorts = Vec::new();
    collect_replacement_sorts(left, replacements, &mut sorts);
    collect_replacement_sorts(right, replacements, &mut sorts);
    if !sorts.is_empty() && sorts.iter().all(|sort| *sort == Sort::Int) {
        Sort::Int
    } else {
        Sort::Number
    }
}

fn collect_replacement_sorts(
    predicate: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    sorts: &mut Vec<Sort>,
) {
    match predicate {
        PredicateExpr::Identifier(name) => {
            if let Some(term) = replacements.get(name) {
                sorts.push(term.sort());
            }
        }
        PredicateExpr::Return => {
            if let Some(term) = replacements.get("$") {
                sorts.push(term.sort());
            }
        }
        PredicateExpr::Member(object, _)
        | PredicateExpr::Not(object)
        | PredicateExpr::PredicateApply(_, object) => {
            collect_replacement_sorts(object, replacements, sorts);
        }
        PredicateExpr::Binary(_, left, right) | PredicateExpr::Logical(_, left, right) => {
            collect_replacement_sorts(left, replacements, sorts);
            collect_replacement_sorts(right, replacements, sorts);
        }
        PredicateExpr::Literal(_) => {}
    }
}

fn predicate_literal_sort(predicate: &PredicateExpr) -> Option<Sort> {
    match predicate {
        PredicateExpr::Literal(Literal::Boolean(_)) => Some(Sort::Bool),
        PredicateExpr::Literal(Literal::Number(_)) => Some(Sort::Number),
        PredicateExpr::Literal(Literal::String(_)) => Some(Sort::String),
        _ => None,
    }
}

fn is_numeric_sort(sort: Sort) -> bool {
    matches!(sort, Sort::Number | Sort::Int)
}

fn predicate_binary(operator: &BinaryOp, left: Term, right: Term) -> Result<Term, String> {
    let left_sort = left.sort();
    let right_sort = right.sort();
    Ok(match operator {
        BinaryOp::EqEqEq | BinaryOp::EqEq
            if left_sort == right_sort
                || (is_numeric_sort(left_sort) && is_numeric_sort(right_sort)) =>
        {
            Term::Eq(Box::new(left), Box::new(right))
        }
        BinaryOp::NotEqEq | BinaryOp::NotEq
            if left_sort == right_sort
                || (is_numeric_sort(left_sort) && is_numeric_sort(right_sort)) =>
        {
            Term::Ne(Box::new(left), Box::new(right))
        }
        BinaryOp::Gt if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Gt(Box::new(left), Box::new(right))
        }
        BinaryOp::Lt if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Lt(Box::new(left), Box::new(right))
        }
        BinaryOp::Gte if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Ge(Box::new(left), Box::new(right))
        }
        BinaryOp::Lte if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Le(Box::new(left), Box::new(right))
        }
        BinaryOp::Add if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Add(Box::new(left), Box::new(right))
        }
        BinaryOp::Sub if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Sub(Box::new(left), Box::new(right))
        }
        BinaryOp::Mul if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Mul(Box::new(left), Box::new(right))
        }
        BinaryOp::Div => {
            return Err("Division is outside the integer refinement subset".into());
        }
        _ => return Err("Ill-typed refinement predicate".into()),
    })
}

fn binary_term(operator: BinaryOperator, left: Term, right: Term) -> Result<Term, String> {
    let left_sort = left.sort();
    let right_sort = right.sort();
    Ok(match operator {
        BinaryOperator::Equality | BinaryOperator::StrictEquality
            if left_sort == right_sort
                || (is_numeric_sort(left_sort) && is_numeric_sort(right_sort)) =>
        {
            Term::Eq(Box::new(left), Box::new(right))
        }
        BinaryOperator::Inequality | BinaryOperator::StrictInequality
            if left_sort == right_sort
                || (is_numeric_sort(left_sort) && is_numeric_sort(right_sort)) =>
        {
            Term::Ne(Box::new(left), Box::new(right))
        }
        BinaryOperator::GreaterThan
            if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) =>
        {
            Term::Gt(Box::new(left), Box::new(right))
        }
        BinaryOperator::LessThan if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Lt(Box::new(left), Box::new(right))
        }
        BinaryOperator::GreaterEqualThan
            if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) =>
        {
            Term::Ge(Box::new(left), Box::new(right))
        }
        BinaryOperator::LessEqualThan
            if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) =>
        {
            Term::Le(Box::new(left), Box::new(right))
        }
        BinaryOperator::Addition if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) => {
            Term::Add(Box::new(left), Box::new(right))
        }
        BinaryOperator::Subtraction
            if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) =>
        {
            Term::Sub(Box::new(left), Box::new(right))
        }
        BinaryOperator::Multiplication
            if is_numeric_sort(left_sort) && is_numeric_sort(right_sort) =>
        {
            Term::Mul(Box::new(left), Box::new(right))
        }
        BinaryOperator::Division => {
            return Err("JavaScript division is outside the integer refinement subset".into());
        }
        _ => {
            return Err(format!(
                "Unsupported or ill-typed binary operator '{}'",
                operator.as_str()
            ));
        }
    })
}

fn substitute(term: &Term, target: &Term, replacement: &Term) -> Term {
    if term == target {
        return replacement.clone();
    }
    match term {
        Term::Add(left, right) => Term::Add(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Sub(left, right) => Term::Sub(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Mul(left, right) => Term::Mul(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Same(left, right) => Term::Same(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Eq(left, right) => Term::Eq(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Ne(left, right) => Term::Ne(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Gt(left, right) => Term::Gt(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Lt(left, right) => Term::Lt(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Ge(left, right) => Term::Ge(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Le(left, right) => Term::Le(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::And(left, right) => Term::And(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Or(left, right) => Term::Or(
            Box::new(substitute(left, target, replacement)),
            Box::new(substitute(right, target, replacement)),
        ),
        Term::Not(inner) => Term::Not(Box::new(substitute(inner, target, replacement))),
        Term::ToNumber(inner) => Term::ToNumber(Box::new(substitute(inner, target, replacement))),
        Term::Pred(name, argument) => Term::Pred(
            name.clone(),
            Box::new(substitute(argument, target, replacement)),
        ),
        Term::Member(object, property, sort) => Term::Member(
            Box::new(substitute(object, target, replacement)),
            property.clone(),
            *sort,
        ),
        Term::Index(object, index, sort) => Term::Index(
            Box::new(substitute(object, target, replacement)),
            Box::new(substitute(index, target, replacement)),
            *sort,
        ),
        other => other.clone(),
    }
}

fn contains_term(term: &Term, needle: &Term) -> bool {
    if term == needle {
        return true;
    }
    match term {
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right)
        | Term::Index(left, right, _) => {
            contains_term(left, needle) || contains_term(right, needle)
        }
        Term::Not(inner)
        | Term::Pred(_, inner)
        | Term::Member(inner, _, _)
        | Term::ToNumber(inner) => contains_term(inner, needle),
        Term::Number(_) | Term::Int(_) | Term::Bool(_) | Term::String(_) | Term::Var(_, _) => false,
    }
}

fn fact_indexes_object(fact: &Term, object: &Term) -> bool {
    match fact {
        Term::Same(left, right) | Term::Eq(left, right) => {
            matches!(left.as_ref(), Term::Index(found, _, _) if found.as_ref() == object)
                || matches!(right.as_ref(), Term::Index(found, _, _) if found.as_ref() == object)
        }
        _ => false,
    }
}

fn contains_heap_term(term: &Term) -> bool {
    match term {
        Term::Member(..) | Term::Index(..) => true,
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right) => contains_heap_term(left) || contains_heap_term(right),
        Term::Not(inner) | Term::Pred(_, inner) | Term::ToNumber(inner) => {
            contains_heap_term(inner)
        }
        Term::Number(_) | Term::Int(_) | Term::Bool(_) | Term::String(_) | Term::Var(_, _) => false,
    }
}

fn known_number_equality(assumptions: &[Term], target: &Term) -> Option<i64> {
    assumptions
        .iter()
        .find_map(|assumption| known_number_equality_in_term(assumption, target))
}

fn known_number_equality_in_term(term: &Term, target: &Term) -> Option<i64> {
    match term {
        Term::Same(left, right) | Term::Eq(left, right) => match (&**left, &**right) {
            (left, Term::Number(value)) if left == target => Some(*value),
            (Term::Number(value), right) if right == target => Some(*value),
            (left, Term::Int(value)) if left == target => Some(*value),
            (Term::Int(value), right) if right == target => Some(*value),
            _ => None,
        },
        Term::And(left, right) => known_number_equality_in_term(left, target)
            .or_else(|| known_number_equality_in_term(right, target)),
        _ => None,
    }
}

/// Preserve facts about one pre-call heap measure under a scalar snapshot,
/// then forget heap-dependent facts which a native callback or mutation may
/// invalidate. Intrinsic post-call facts (for example, `length >= 0`) are
/// restored from the values still in scope.
fn snapshot_heap_measure(state: &mut State, measure: &Term, snapshot: &Term) {
    let snapshot_facts = state
        .assumptions
        .iter()
        .filter(|fact| contains_term(fact, measure))
        .map(|fact| substitute(fact, measure, snapshot))
        .flat_map(split_conjuncts)
        .filter(|fact| !contains_heap_term(fact))
        .collect::<Vec<_>>();
    let index_facts = match measure {
        Term::Member(object, property, _) if property == "length" => state
            .assumptions
            .iter()
            .cloned()
            .flat_map(split_conjuncts)
            .filter(|fact| fact_indexes_object(fact, object))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    invalidate_heap_facts(state);
    state.assumptions.extend(snapshot_facts);
    state.assumptions.extend(index_facts);
}

fn split_conjuncts(term: Term) -> Vec<Term> {
    match term {
        Term::And(left, right) => split_conjuncts(*left)
            .into_iter()
            .chain(split_conjuncts(*right))
            .collect(),
        term => vec![term],
    }
}

fn invalidate_heap_facts(state: &mut State) {
    state.assumptions.retain(|fact| !contains_heap_term(fact));
    for value in state
        .env
        .values_mut()
        .chain(state.compiler_bindings.values_mut())
        .chain(
            state
                .scopes
                .iter_mut()
                .flat_map(|scope| scope.values_mut())
                .filter_map(|(value, _)| value.as_mut()),
        )
    {
        if value
            .qualifier
            .as_ref()
            .is_some_and(|qualifier| contains_heap_term(&qualifier.formula))
        {
            value.qualifier = None;
        }
    }
    let intrinsic_facts = state
        .env
        .values()
        .chain(state.compiler_bindings.values())
        .chain(
            state
                .scopes
                .iter()
                .flat_map(|scope| scope.values())
                .filter_map(|(value, _)| value.as_ref()),
        )
        .filter(|value| value.catalog_trusted)
        .flat_map(|value| intrinsic_refinements(&value.base, &value.term))
        .collect::<Vec<_>>();
    state.assumptions.extend(intrinsic_facts);
}

fn record_reference_provenance_edges(term: &Term, edges: &mut Vec<ReferenceProvenanceEdge>) {
    match term {
        Term::Same(left, right) if left.sort() == Sort::Ref && right.sort() == Sort::Ref => {
            record_reference_alias(left, right, edges);
            record_reference_containment(left, edges);
            record_reference_containment(right, edges);
        }
        Term::And(left, right) => {
            record_reference_provenance_edges(left, edges);
            record_reference_provenance_edges(right, edges);
        }
        _ => {}
    }
}

fn merge_reference_provenance_edges(
    target: &mut Vec<ReferenceProvenanceEdge>,
    source: &[ReferenceProvenanceEdge],
) {
    for edge in source {
        if !target.contains(edge) {
            target.push(edge.clone());
        }
    }
}

fn record_reference_alias(left: &Term, right: &Term, edges: &mut Vec<ReferenceProvenanceEdge>) {
    let edge = ReferenceProvenanceEdge::Alias(left.clone(), right.clone());
    let reverse = ReferenceProvenanceEdge::Alias(right.clone(), left.clone());
    if !edges.contains(&edge) && !edges.contains(&reverse) {
        edges.push(edge);
    }
}

fn record_reference_containment(term: &Term, edges: &mut Vec<ReferenceProvenanceEdge>) {
    let container = match term {
        Term::Member(container, _, Sort::Ref) | Term::Index(container, _, Sort::Ref)
            if container.sort() == Sort::Ref =>
        {
            container.as_ref()
        }
        _ => return,
    };
    record_reference_containment_edge(term, container, edges);
    record_reference_containment(container, edges);
}

fn record_reference_containment_edge(
    value: &Term,
    container: &Term,
    edges: &mut Vec<ReferenceProvenanceEdge>,
) {
    if value.sort() != Sort::Ref || container.sort() != Sort::Ref {
        return;
    }
    let edge = ReferenceProvenanceEdge::ContainedBy {
        value: value.clone(),
        container: container.clone(),
    };
    if !edges.contains(&edge) {
        edges.push(edge);
    }
}

fn reference_aliases(state: &State, receiver: &Term) -> HashSet<Term> {
    let mut aliases = HashSet::from([receiver.clone()]);
    loop {
        let mut changed = false;
        for assumption in &state.assumptions {
            let Term::Same(left, right) = assumption else {
                continue;
            };
            if left.sort() != Sort::Ref || right.sort() != Sort::Ref {
                continue;
            }
            if aliases.contains(left.as_ref()) {
                changed |= aliases.insert(right.as_ref().clone());
            }
            if aliases.contains(right.as_ref()) {
                changed |= aliases.insert(left.as_ref().clone());
            }
        }
        for edge in &state.provenance_edges {
            let ReferenceProvenanceEdge::Alias(left, right) = edge else {
                continue;
            };
            if aliases.contains(left) {
                changed |= aliases.insert(right.clone());
            }
            if aliases.contains(right) {
                changed |= aliases.insert(left.clone());
            }
        }
        if !changed {
            break;
        }
    }
    aliases
}

fn reference_provenance_targets(state: &State, source: &Term) -> HashSet<Term> {
    let mut targets = reference_aliases(state, source);
    loop {
        let mut changed = false;
        for edge in &state.provenance_edges {
            let ReferenceProvenanceEdge::ContainedBy { value, container } = edge else {
                continue;
            };
            if targets.contains(value) {
                changed |= targets.insert(container.clone());
            }
        }
        let current = targets.iter().cloned().collect::<Vec<_>>();
        for target in current {
            for alias in reference_aliases(state, &target) {
                changed |= targets.insert(alias);
            }
        }
        if !changed {
            break;
        }
    }
    targets
}

fn reference_alias_has_local_implementation(state: &State, receiver: &Term) -> bool {
    let aliases = reference_aliases(state, receiver);
    state
        .local_reference_provenance
        .iter()
        .any(|reference| aliases.contains(reference))
        || state
            .env
            .values()
            .chain(state.compiler_bindings.values())
            .chain(
                state
                    .scopes
                    .iter()
                    .flat_map(|scope| scope.values())
                    .filter_map(|(value, _)| value.as_ref()),
            )
            .any(|value| {
                value.local_implementation
                    && value.term.sort() == Sort::Ref
                    && aliases.contains(&value.term)
            })
}

fn mark_reference_aliases_as_local(state: &mut State, receiver: &Term) {
    let targets = reference_provenance_targets(state, receiver);
    state
        .local_reference_provenance
        .extend(targets.iter().cloned());
    for value in state
        .env
        .values_mut()
        .chain(state.compiler_bindings.values_mut())
        .chain(
            state
                .scopes
                .iter_mut()
                .flat_map(|scope| scope.values_mut())
                .filter_map(|(value, _)| value.as_mut()),
        )
    {
        if value.term.sort() == Sort::Ref && targets.contains(&value.term) {
            value.local_implementation = true;
        }
    }
}

fn taint_implementation_capable_references(state: &mut State) {
    for value in state
        .env
        .values_mut()
        .chain(state.compiler_bindings.values_mut())
        .chain(
            state
                .scopes
                .iter_mut()
                .flat_map(|scope| scope.values_mut())
                .filter_map(|(value, _)| value.as_mut()),
        )
    {
        if value.term.sort() == Sort::Ref && base_may_contain_local_implementation(&value.base) {
            value.local_implementation = true;
        }
    }
}

fn havoc_value(verifier: &mut Verifier<'_>, name: &str, value: &mut Value) -> Term {
    let term = Term::Var(
        verifier.fresh_name(&format!("effect_havoc_{name}")),
        sort_for_base(&value.base),
    );
    value.term = term.clone();
    value.qualifier = None;
    // A reassignable binding can cross an `any`-typed callback and come back
    // with a runtime value that does not have its statically rendered standard-
    // library identity. Keep the base only for ordinary type compatibility;
    // it can no longer justify catalog refinements such as Array.length >= 0.
    value.catalog_trusted = false;
    // The same boundary can replace a mutable reference with a project-authored
    // implementation whose methods merely satisfy a declaration shape. Scalar
    // values cannot carry such an implementation, so do not taint them.
    value.local_implementation |= base_may_contain_local_implementation(&value.base);
    term
}

fn havoc_callback_value(verifier: &mut Verifier<'_>, name: &str, value: &mut Value) -> Term {
    let term = if value.term.sort() == Sort::Ref {
        // Entering a checked callback cannot rebind a reference unless the
        // callback contains an assignment, which this subset rejects. Keep
        // aliases intact while invalidating heap facts below.
        value.term.clone()
    } else {
        Term::Var(
            verifier.fresh_name(&format!("callback_havoc_{name}")),
            sort_for_base(&value.base),
        )
    };
    value.term = term.clone();
    value.qualifier = None;
    // The callback is checked synchronously. Its captured binding may change,
    // but merely entering the callback does not replace a declaration-backed
    // object with a project implementation or revoke its catalog identity.
    term
}

fn signature_accepts_arity(signature: &FunctionSignature, argument_count: usize) -> bool {
    let required = signature
        .parameters
        .iter()
        .filter(|parameter| !parameter.optional && !parameter.rest)
        .count();
    let has_rest = signature.parameters.iter().any(|parameter| parameter.rest);
    argument_count >= required && (has_rest || argument_count <= signature.parameters.len())
}

fn catalog_errors_allow_compiler_fallback(errors: &[RtError]) -> bool {
    !errors.is_empty()
        && errors.iter().all(|error| {
            error.message.starts_with("Base type mismatch:")
                || error.message.starts_with("Callback return type mismatch:")
        })
}

fn contextual_arrow<'a>(
    expression: &'a Expression<'a>,
) -> Option<&'a oxc_ast::ast::ArrowFunctionExpression<'a>> {
    match expression {
        Expression::ArrowFunctionExpression(arrow) => Some(arrow),
        Expression::ParenthesizedExpression(parenthesized) => {
            contextual_arrow(&parenthesized.expression)
        }
        _ => None,
    }
}

fn contextual_object_literal<'a>(
    expression: &'a Expression<'a>,
) -> Option<&'a oxc_ast::ast::ObjectExpression<'a>> {
    match expression {
        Expression::ObjectExpression(object) => Some(object),
        Expression::ParenthesizedExpression(parenthesized) => {
            contextual_object_literal(&parenthesized.expression)
        }
        _ => None,
    }
}

fn compiler_expression_is_pure_creation(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ObjectExpression(object) => object.properties.iter().all(|property| {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                return false;
            };
            !property.computed
                && (property.method
                    || property.kind != PropertyKind::Init
                    || compiler_expression_is_pure_creation(&property.value))
        }),
        Expression::ParenthesizedExpression(parenthesized) => {
            compiler_expression_is_pure_creation(&parenthesized.expression)
        }
        Expression::Identifier(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::FunctionExpression(_) => true,
        _ => false,
    }
}

fn contract_has_callable_preconditions(contract: &Contract) -> bool {
    contract.params.iter().any(|(_, parameter)| {
        parameter.predicate.is_some()
            || base_requires_catalog_identity(&parameter.base)
            || base_contains_callable_preconditions(&parameter.base)
    }) || base_contains_callable_preconditions(&contract.ret.base)
}

fn base_contains_callable_preconditions(base: &BaseType) -> bool {
    match base {
        BaseType::Function(parameters, returns) => {
            parameters.iter().any(|parameter| {
                parameter.ty.predicate.is_some()
                    || base_requires_catalog_identity(&parameter.ty.base)
                    || base_contains_callable_preconditions(&parameter.ty.base)
            }) || base_contains_callable_preconditions(&returns.base)
        }
        BaseType::Array(element) => base_contains_callable_preconditions(element),
        BaseType::Generic(_, arguments) | BaseType::Union(arguments) => {
            arguments.iter().any(base_contains_callable_preconditions)
        }
        BaseType::Object(fields) => fields
            .iter()
            .any(|(_, field)| base_contains_callable_preconditions(field)),
        BaseType::Primitive(_) | BaseType::Named(_) | BaseType::Omitted => false,
    }
}

fn record_library_argument_provenance(
    expected: &BaseType,
    value: &Value,
    bindings: &mut HashMap<String, bool>,
) {
    if let BaseType::Function(_, returns) = expected {
        let return_is_reference = match &value.base {
            BaseType::Function(_, actual_returns) => {
                base_may_be_reference_identity(&actual_returns.base)
            }
            _ => true,
        };
        record_type_variable_provenance(
            &returns.base,
            value.local_implementation && return_is_reference,
            bindings,
        );
    } else {
        record_type_variable_provenance(expected, value.local_implementation, bindings);
    }
}

fn record_type_variable_provenance(
    base: &BaseType,
    local_implementation: bool,
    bindings: &mut HashMap<String, bool>,
) {
    match base {
        BaseType::Named(name) if name.starts_with('$') => {
            bindings
                .entry(name.clone())
                .and_modify(|bound| *bound |= local_implementation)
                .or_insert(local_implementation);
        }
        BaseType::Array(element) => {
            record_type_variable_provenance(element, local_implementation, bindings);
        }
        BaseType::Generic(_, arguments) | BaseType::Union(arguments) => {
            for argument in arguments {
                record_type_variable_provenance(argument, local_implementation, bindings);
            }
        }
        BaseType::Object(fields) => {
            for (_, field) in fields {
                record_type_variable_provenance(field, local_implementation, bindings);
            }
        }
        BaseType::Function(parameters, returns) => {
            for parameter in parameters {
                record_type_variable_provenance(&parameter.ty.base, local_implementation, bindings);
            }
            record_type_variable_provenance(&returns.base, local_implementation, bindings);
        }
        BaseType::Primitive(_) | BaseType::Named(_) | BaseType::Omitted => {}
    }
}

fn type_variable_local_implementation(base: &BaseType, bindings: &HashMap<String, bool>) -> bool {
    match base {
        BaseType::Named(name) if name.starts_with('$') => {
            bindings.get(name).copied().unwrap_or(true)
        }
        BaseType::Array(element) => type_variable_local_implementation(element, bindings),
        BaseType::Generic(_, arguments) | BaseType::Union(arguments) => arguments
            .iter()
            .any(|argument| type_variable_local_implementation(argument, bindings)),
        BaseType::Object(fields) => fields
            .iter()
            .any(|(_, field)| type_variable_local_implementation(field, bindings)),
        BaseType::Function(parameters, returns) => {
            parameters
                .iter()
                .any(|parameter| type_variable_local_implementation(&parameter.ty.base, bindings))
                || type_variable_local_implementation(&returns.base, bindings)
        }
        BaseType::Primitive(_) | BaseType::Named(_) | BaseType::Omitted => false,
    }
}

fn base_may_contain_local_implementation(base: &BaseType) -> bool {
    match base {
        BaseType::Primitive(name) => matches!(name.as_str(), "any" | "unknown" | "object"),
        BaseType::Array(element) => base_may_contain_local_implementation(element),
        BaseType::Generic(name, arguments)
            if matches!(name.as_str(), "DenseArray" | "ReadonlyArray") =>
        {
            arguments.iter().any(base_may_contain_local_implementation)
        }
        BaseType::Union(members) => members.iter().any(base_may_contain_local_implementation),
        BaseType::Generic(_, _)
        | BaseType::Object(_)
        | BaseType::Function(_, _)
        | BaseType::Named(_) => true,
        BaseType::Omitted => false,
    }
}

fn base_may_be_reference_identity(base: &BaseType) -> bool {
    match base {
        BaseType::Primitive(name) => matches!(name.as_str(), "any" | "unknown" | "object"),
        BaseType::Union(members) => members.iter().any(base_may_be_reference_identity),
        BaseType::Array(_)
        | BaseType::Generic(_, _)
        | BaseType::Object(_)
        | BaseType::Function(_, _)
        | BaseType::Named(_) => true,
        BaseType::Omitted => false,
    }
}

fn compiler_span_is_escapable_reference(hints: Option<&CompilerHints>, span: Span) -> bool {
    let Some(rendered) = hints
        .and_then(|hints| hints.get(span))
        .and_then(|hint| hint.rendered_type.as_deref())
    else {
        return false;
    };
    let base = parse_typescript_type(rendered);
    sort_for_base(&base) == Sort::Ref
        && !matches!(base, BaseType::Function(_, _))
        && base_may_contain_local_implementation(&base)
}

fn signature_arity_description(signature: &FunctionSignature) -> String {
    let required = signature
        .parameters
        .iter()
        .filter(|parameter| !parameter.optional && !parameter.rest)
        .count();
    if signature.parameters.iter().any(|parameter| parameter.rest) {
        return format!("at least {required}");
    }
    let maximum = signature.parameters.len();
    if required == maximum {
        required.to_string()
    } else {
        format!("{required} to {maximum}")
    }
}

fn signature_parameter(
    signature: &FunctionSignature,
    argument_index: usize,
) -> Option<&crate::prelude::LibraryParameter> {
    signature.parameters.get(argument_index).or_else(|| {
        signature
            .parameters
            .last()
            .filter(|parameter| parameter.rest)
    })
}

fn constraint_variables(constraint: &FixpointConstraint<'_>) -> Vec<(String, Sort)> {
    let mut variables = std::collections::BTreeMap::new();
    for term in constraint
        .assumptions
        .iter()
        .chain(std::iter::once(constraint.consequent))
    {
        collect_variables(term, &mut variables);
    }
    variables.into_iter().collect()
}

fn collect_variables(term: &Term, output: &mut std::collections::BTreeMap<String, Sort>) {
    match term {
        Term::Var(name, sort) => {
            output.insert(name.clone(), *sort);
        }
        Term::Add(left, right)
        | Term::Sub(left, right)
        | Term::Mul(left, right)
        | Term::Same(left, right)
        | Term::Eq(left, right)
        | Term::Ne(left, right)
        | Term::Gt(left, right)
        | Term::Lt(left, right)
        | Term::Ge(left, right)
        | Term::Le(left, right)
        | Term::And(left, right)
        | Term::Or(left, right)
        | Term::Index(left, right, _) => {
            collect_variables(left, output);
            collect_variables(right, output);
        }
        Term::Not(inner)
        | Term::Pred(_, inner)
        | Term::Member(inner, _, _)
        | Term::ToNumber(inner) => collect_variables(inner, output),
        Term::Number(_) | Term::Int(_) | Term::Bool(_) | Term::String(_) => {}
    }
}

enum ZTerm {
    Number(Float),
    Int(Z3Int),
    Bool(Bool),
    String(Z3String),
    Ref(Dynamic),
}

fn to_z3(term: &Term) -> Result<ZTerm, String> {
    match term {
        Term::Number(value) => Ok(ZTerm::Number(Float::from_f64(*value as f64))),
        Term::Int(value) => Ok(ZTerm::Int(Z3Int::from_i64(*value))),
        Term::ToNumber(inner) => match to_z3(inner)? {
            ZTerm::Int(value) => int_to_number(&value),
            ZTerm::Number(value) => Ok(ZTerm::Number(value)),
            _ => Err("toNumber requires a logical integer".into()),
        },
        Term::Bool(value) => Ok(ZTerm::Bool(Bool::from_bool(*value))),
        Term::String(value) => Z3String::from_str(value)
            .map(ZTerm::String)
            .map_err(|_| "String literal contains a null byte".into()),
        Term::Var(name, Sort::Number) => Ok(ZTerm::Number(Float::new_const_double(name.as_str()))),
        Term::Var(name, Sort::Int) => Ok(ZTerm::Int(Z3Int::new_const(name.as_str()))),
        Term::Var(name, Sort::Bool) => Ok(ZTerm::Bool(Bool::new_const(name.as_str()))),
        Term::Var(name, Sort::String) => Ok(ZTerm::String(Z3String::new_const(name.as_str()))),
        Term::Var(name, Sort::Ref) => {
            let sort = z3_sort(Sort::Ref);
            Ok(ZTerm::Ref(Dynamic::new_const(name.as_str(), &sort)))
        }
        Term::Member(object, property, result_sort) => {
            let object_sort = object.sort();
            let object = zterm_dynamic(to_z3(object)?);
            let domain = z3_sort(object_sort);
            let range = z3_sort(*result_sort);
            let name = format!(
                "__rt_member_{}_{}_{}",
                sort_name(object_sort),
                sort_name(*result_sort),
                property
            );
            let declaration = FuncDecl::new(name, &[&domain], &range);
            zterm_from_dynamic(declaration.apply(&[&object]), *result_sort)
        }
        Term::Index(object, index, result_sort) => {
            let object_sort = object.sort();
            let index_sort = index.sort();
            let object = zterm_dynamic(to_z3(object)?);
            let index = zterm_dynamic(to_z3(index)?);
            let object_domain = z3_sort(object_sort);
            let index_domain = z3_sort(index_sort);
            let range = z3_sort(*result_sort);
            let name = format!(
                "__rt_index_{}_{}_{}",
                sort_name(object_sort),
                sort_name(index_sort),
                sort_name(*result_sort)
            );
            let declaration = FuncDecl::new(name, &[&object_domain, &index_domain], &range);
            zterm_from_dynamic(declaration.apply(&[&object, &index]), *result_sort)
        }
        Term::Pred(name, argument) => match to_z3(argument)? {
            ZTerm::Number(argument) => {
                let domain = Z3Sort::double();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
            ZTerm::Bool(argument) => {
                let domain = Z3Sort::bool();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
            ZTerm::String(argument) => {
                let domain = Z3Sort::string();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
            ZTerm::Int(argument) => {
                let domain = Z3Sort::int();
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
            ZTerm::Ref(argument) => {
                let domain = z3_sort(Sort::Ref);
                let range = Z3Sort::bool();
                let predicate = FuncDecl::new(name.as_str(), &[&domain], &range);
                Ok(ZTerm::Bool(
                    predicate.apply(&[&argument]).as_bool().unwrap(),
                ))
            }
        },
        Term::Add(left, right) => arithmetic_pair(
            left,
            right,
            |a, b| a + b,
            |a, b| a.add_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even()),
        ),
        Term::Sub(left, right) => arithmetic_pair(
            left,
            right,
            |a, b| a - b,
            |a, b| a.sub_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even()),
        ),
        Term::Mul(left, right) => arithmetic_pair(
            left,
            right,
            |a, b| a * b,
            |a, b| a.mul_with_rounding_mode(b, &RoundingMode::round_nearest_ties_to_even()),
        ),
        Term::Same(left, right) => structural_equality(left, right),
        Term::Eq(left, right) => equality(left, right, false),
        Term::Ne(left, right) => equality(left, right, true),
        Term::Gt(left, right) => ordered_compare(left, right, |a, b| a.gt(b), |a, b| a.gt(b)),
        Term::Lt(left, right) => ordered_compare(left, right, |a, b| a.lt(b), |a, b| a.lt(b)),
        Term::Ge(left, right) => ordered_compare(left, right, |a, b| a.ge(b), |a, b| a.ge(b)),
        Term::Le(left, right) => ordered_compare(left, right, |a, b| a.le(b), |a, b| a.le(b)),
        Term::And(left, right) => bool_pair(left, right, |a, b| Bool::and(&[a, b])),
        Term::Or(left, right) => bool_pair(left, right, |a, b| Bool::or(&[a, b])),
        Term::Not(inner) => match to_z3(inner)? {
            ZTerm::Bool(value) => Ok(ZTerm::Bool(value.not())),
            _ => Err("Logical not requires a boolean".into()),
        },
    }
}

fn sort_name(sort: Sort) -> &'static str {
    match sort {
        Sort::Number => "number",
        Sort::Int => "int",
        Sort::Bool => "bool",
        Sort::String => "string",
        Sort::Ref => "ref",
    }
}

fn z3_sort(sort: Sort) -> Z3Sort {
    match sort {
        Sort::Number => Z3Sort::double(),
        Sort::Int => Z3Sort::int(),
        Sort::Bool => Z3Sort::bool(),
        Sort::String => Z3Sort::string(),
        Sort::Ref => Z3Sort::uninterpreted(Symbol::String("Ref".into())),
    }
}

fn zterm_dynamic(term: ZTerm) -> Dynamic {
    match term {
        ZTerm::Number(value) => Dynamic::from_ast(&value),
        ZTerm::Int(value) => Dynamic::from_ast(&value),
        ZTerm::Bool(value) => Dynamic::from_ast(&value),
        ZTerm::String(value) => Dynamic::from_ast(&value),
        ZTerm::Ref(value) => value,
    }
}

fn zterm_from_dynamic(term: Dynamic, sort: Sort) -> Result<ZTerm, String> {
    match sort {
        Sort::Number => term
            .as_float()
            .map(ZTerm::Number)
            .ok_or_else(|| "Member result is not a number".into()),
        Sort::Int => term
            .as_int()
            .map(ZTerm::Int)
            .ok_or_else(|| "Member result is not a logical integer".into()),
        Sort::Bool => term
            .as_bool()
            .map(ZTerm::Bool)
            .ok_or_else(|| "Member result is not a boolean".into()),
        Sort::String => term
            .as_string()
            .map(ZTerm::String)
            .ok_or_else(|| "Member result is not a string".into()),
        Sort::Ref => Ok(ZTerm::Ref(term)),
    }
}

fn int_to_number(value: &Z3Int) -> Result<ZTerm, String> {
    let domain = Z3Sort::int();
    let range = Z3Sort::double();
    let declaration = FuncDecl::new("__rt_int_to_number", &[&domain], &range);
    let applied = declaration.apply(&[&Dynamic::from_ast(value)]);
    applied
        .as_float()
        .map(ZTerm::Number)
        .ok_or_else(|| "int-to-number conversion did not produce a float".into())
}

fn arithmetic_pair(
    left: &Term,
    right: &Term,
    int_op: impl FnOnce(Z3Int, Z3Int) -> Z3Int,
    float_op: impl FnOnce(Float, Float) -> Float,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Int(left), ZTerm::Int(right)) => Ok(ZTerm::Int(int_op(left, right))),
        (left, right) => {
            let left = zterm_as_number(left)?;
            let right = zterm_as_number(right)?;
            Ok(ZTerm::Number(float_op(left, right)))
        }
    }
}

fn ordered_compare(
    left: &Term,
    right: &Term,
    int_op: impl FnOnce(&Z3Int, &Z3Int) -> Bool,
    float_op: impl FnOnce(&Float, &Float) -> Bool,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Int(left), ZTerm::Int(right)) => Ok(ZTerm::Bool(int_op(&left, &right))),
        (left, right) => {
            let left = zterm_as_number(left)?;
            let right = zterm_as_number(right)?;
            Ok(ZTerm::Bool(float_op(&left, &right)))
        }
    }
}

fn zterm_as_number(term: ZTerm) -> Result<Float, String> {
    match term {
        ZTerm::Number(value) => Ok(value),
        ZTerm::Int(value) => match int_to_number(&value)? {
            ZTerm::Number(value) => Ok(value),
            _ => Err("int-to-number conversion did not produce a float".into()),
        },
        _ => Err("Ordered comparison requires number or logical integer operands".into()),
    }
}

#[allow(dead_code)]
fn as_number_term(term: &Term) -> Term {
    match term.sort() {
        Sort::Number => term.clone(),
        Sort::Int => match term {
            Term::Int(value) => Term::Number(*value),
            _ => Term::ToNumber(Box::new(term.clone())),
        },
        _ => term.clone(),
    }
}

fn as_int_term(term: &Term) -> Option<Term> {
    match term {
        Term::Int(_) => Some(term.clone()),
        Term::Number(value) => Some(Term::Int(*value)),
        Term::Var(_, Sort::Int)
        | Term::Member(_, _, Sort::Int)
        | Term::Index(_, _, Sort::Int)
        | Term::Add(_, _)
        | Term::Sub(_, _)
        | Term::Mul(_, _)
            if term.sort() == Sort::Int =>
        {
            Some(term.clone())
        }
        _ => None,
    }
}

fn collection_length(term: &Term) -> Term {
    Term::Member(Box::new(term.clone()), "length".into(), Sort::Int)
}

fn is_dense_array(base: &BaseType) -> bool {
    matches!(base, BaseType::Generic(name, arguments) if name == "DenseArray" && arguments.len() == 1)
}

fn is_boolean_base(base: &BaseType) -> bool {
    matches!(base, BaseType::Primitive(name) if name == "boolean")
}

fn sort_for_indexed_type(ty: &RefinementType) -> Sort {
    if ty.index.is_some() {
        match &ty.base {
            BaseType::Primitive(name) if name == "number" => Sort::Int,
            BaseType::Primitive(name) if name == "boolean" => Sort::Bool,
            _ => sort_for_base(&ty.base),
        }
    } else {
        sort_for_base(&ty.base)
    }
}

fn index_formula(value: &Value, index_term: &Term) -> Term {
    if is_dense_array(&value.base) {
        let length = collection_length(&value.term);
        let expected = as_int_term(index_term).unwrap_or_else(|| index_term.clone());
        Term::Eq(Box::new(length), Box::new(expected))
    } else if is_boolean_base(&value.base) && *index_term == Term::Bool(true) {
        value.term.clone()
    } else if is_boolean_base(&value.base) && *index_term == Term::Bool(false) {
        Term::Not(Box::new(value.term.clone()))
    } else {
        Term::Eq(Box::new(value.term.clone()), Box::new(index_term.clone()))
    }
}

fn int_bound_term(state: &State, name: &str) -> Option<Term> {
    state
        .env
        .get(name)
        .map(|value| value.term.clone())
        .or_else(|| state.entry_params.get(name).cloned())
        .filter(|term| term.sort() == Sort::Int)
}

#[derive(Debug, Clone, PartialEq)]
enum LoopCandidate {
    ZeroLe(String),
    OneLe(String),
    LtLength {
        index: String,
        array: String,
    },
    LeLength {
        index: String,
        array: String,
    },
    LeOther {
        left: String,
        right: String,
    },
    EqOther {
        left: String,
        right: String,
    },
    EqDiff {
        left: String,
        right: String,
        minus: String,
    },
    Post {
        name: String,
        predicate: PredicateExpr,
    },
}

impl LoopCandidate {
    fn instantiate(&self, state: &State) -> Option<Term> {
        match self {
            Self::ZeroLe(name) => {
                let value = state.env.get(name)?;
                Some(Term::Le(
                    Box::new(zero_for(&value.term)),
                    Box::new(value.term.clone()),
                ))
            }
            Self::OneLe(name) => {
                let value = state.env.get(name)?;
                Some(Term::Le(
                    Box::new(one_for(&value.term)),
                    Box::new(value.term.clone()),
                ))
            }
            Self::LtLength { index, array } => {
                let index = state.env.get(index)?;
                let array = state.env.get(array)?;
                Some(Term::Lt(
                    Box::new(index.term.clone()),
                    Box::new(collection_length(&array.term)),
                ))
            }
            Self::LeLength { index, array } => {
                let index = state.env.get(index)?;
                let array = state.env.get(array)?;
                Some(Term::Le(
                    Box::new(index.term.clone()),
                    Box::new(collection_length(&array.term)),
                ))
            }
            Self::LeOther { left, right } => {
                let left = state.env.get(left)?;
                let right = state
                    .env
                    .get(right)
                    .map(|value| value.term.clone())
                    .or_else(|| state.entry_params.get(right).cloned())?;
                Some(Term::Le(Box::new(left.term.clone()), Box::new(right)))
            }
            Self::EqOther { left, right } => {
                let left = state.env.get(left)?;
                let right = int_bound_term(state, right)?;
                if left.term.sort() != Sort::Int || right.sort() != Sort::Int {
                    return None;
                }
                Some(Term::Eq(Box::new(left.term.clone()), Box::new(right)))
            }
            Self::EqDiff { left, right, minus } => {
                let left = state.env.get(left)?;
                let right = int_bound_term(state, right)?;
                let minus = int_bound_term(state, minus)?;
                if left.term.sort() != Sort::Int {
                    return None;
                }
                Some(Term::Eq(
                    Box::new(left.term.clone()),
                    Box::new(Term::Sub(Box::new(right), Box::new(minus))),
                ))
            }
            Self::Post { name, predicate } => {
                let value = state.env.get(name)?;
                let replacements = HashMap::from([
                    ("$".to_string(), value.term.clone()),
                    (name.clone(), value.term.clone()),
                ]);
                predicate_term(
                    predicate,
                    &replacements,
                    &HashMap::new(),
                    Some(value.term.sort()),
                )
                .ok()
            }
        }
    }
}

fn zero_for(term: &Term) -> Term {
    if term.sort() == Sort::Int {
        Term::Int(0)
    } else {
        Term::Number(0)
    }
}

fn one_for(term: &Term) -> Term {
    if term.sort() == Sort::Int {
        Term::Int(1)
    } else {
        Term::Number(1)
    }
}

fn index_names_in_contract(contract: &Contract) -> HashSet<String> {
    let mut names = HashSet::new();
    for (_, ty) in &contract.params {
        collect_ident_names(ty.index.as_ref(), &mut names);
    }
    collect_ident_names(contract.ret.index.as_ref(), &mut names);
    names.remove("$");
    names
}

fn collect_ident_names(predicate: Option<&PredicateExpr>, names: &mut HashSet<String>) {
    let Some(predicate) = predicate else {
        return;
    };
    match predicate {
        PredicateExpr::Identifier(name) => {
            names.insert(name.clone());
        }
        PredicateExpr::Return => {
            names.insert("$".into());
        }
        PredicateExpr::Member(object, _)
        | PredicateExpr::Not(object)
        | PredicateExpr::PredicateApply(_, object) => collect_ident_names(Some(object), names),
        PredicateExpr::Binary(_, left, right) | PredicateExpr::Logical(_, left, right) => {
            collect_ident_names(Some(left), names);
            collect_ident_names(Some(right), names);
        }
        PredicateExpr::Literal(_) => {}
    }
}

fn scrape_loop_candidates(
    states: &[State],
    assigned: &HashSet<String>,
    current_function: Option<(&str, &Contract)>,
) -> Vec<LoopCandidate> {
    let mut candidates = Vec::new();
    for state in states {
        let arrays = state
            .env
            .iter()
            .filter(|(_, value)| is_dense_array(&value.base))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let int_bounds = state
            .env
            .iter()
            .filter(|(_, value)| value.term.sort() == Sort::Int)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in assigned {
            let Some(value) = state.env.get(name) else {
                continue;
            };
            if !is_numeric_sort(value.term.sort()) {
                continue;
            }
            candidates.push(LoopCandidate::ZeroLe(name.clone()));
            candidates.push(LoopCandidate::OneLe(name.clone()));
            for array in &arrays {
                candidates.push(LoopCandidate::LtLength {
                    index: name.clone(),
                    array: array.clone(),
                });
                candidates.push(LoopCandidate::LeLength {
                    index: name.clone(),
                    array: array.clone(),
                });
            }
            if value.term.sort() == Sort::Int {
                for bound in &int_bounds {
                    if bound != name {
                        candidates.push(LoopCandidate::LeOther {
                            left: name.clone(),
                            right: bound.clone(),
                        });
                        candidates.push(LoopCandidate::EqOther {
                            left: name.clone(),
                            right: bound.clone(),
                        });
                    }
                }
                let unassigned_ints = int_bounds
                    .iter()
                    .filter(|bound| !assigned.contains(*bound))
                    .cloned()
                    .collect::<Vec<_>>();
                for right in assigned {
                    if right == name {
                        continue;
                    }
                    let Some(right_value) = state.env.get(right) else {
                        continue;
                    };
                    if right_value.term.sort() != Sort::Int {
                        continue;
                    }
                    for minus in &unassigned_ints {
                        candidates.push(LoopCandidate::EqDiff {
                            left: name.clone(),
                            right: right.clone(),
                            minus: minus.clone(),
                        });
                    }
                }
            }
            if let Some((_, contract)) = current_function
                && let Some(predicate) = &contract.ret.predicate
                && is_numeric_sort(value.term.sort())
                && is_numeric_sort(sort_for_base(&contract.ret.base))
            {
                candidates.push(LoopCandidate::Post {
                    name: name.clone(),
                    predicate: predicate.clone(),
                });
            }
        }
    }
    candidates.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
    candidates.dedup();
    candidates
}

fn assigned_names_in_statement(statement: &Statement<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_statement(statement, &mut names);
    names
}

fn assigned_names_in_expression(expression: &Expression<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_expression(expression, &mut names);
    names
}

fn collect_assigned_statement(statement: &Statement<'_>, names: &mut HashSet<String>) {
    match statement {
        Statement::BlockStatement(block) => {
            for statement in &block.body {
                collect_assigned_statement(statement, names);
            }
        }
        Statement::IfStatement(if_statement) => {
            collect_assigned_statement(&if_statement.consequent, names);
            if let Some(alternate) = &if_statement.alternate {
                collect_assigned_statement(alternate, names);
            }
        }
        Statement::WhileStatement(while_statement) => {
            collect_assigned_statement(&while_statement.body, names);
        }
        Statement::ForStatement(for_statement) => {
            collect_assigned_statement(&for_statement.body, names);
            if let Some(update) = &for_statement.update {
                collect_assigned_expression(update, names);
            }
        }
        Statement::ExpressionStatement(expression) => {
            collect_assigned_expression(&expression.expression, names);
        }
        Statement::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(identifier) = &declarator.id {
                    names.insert(identifier.name.to_string());
                }
            }
        }
        _ => {}
    }
}

fn collect_assigned_expression(expression: &Expression<'_>, names: &mut HashSet<String>) {
    match expression {
        Expression::AssignmentExpression(assignment) => {
            if let Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier)) =
                assignment.left.as_simple_assignment_target()
            {
                names.insert(identifier.name.to_string());
            }
            collect_assigned_expression(&assignment.right, names);
        }
        Expression::UpdateExpression(update) => {
            if let SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) = &update.argument
            {
                names.insert(identifier.name.to_string());
            }
        }
        Expression::SequenceExpression(sequence) => {
            for expression in &sequence.expressions {
                collect_assigned_expression(expression, names);
            }
        }
        Expression::ParenthesizedExpression(parenthesized) => {
            collect_assigned_expression(&parenthesized.expression, names);
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn number_pair(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(Float, Float) -> Float,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => Ok(ZTerm::Number(operation(left, right))),
        _ => Err("Arithmetic requires number operands".into()),
    }
}

#[allow(dead_code)]
fn number_compare(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(&Float, &Float) -> Bool,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => Ok(ZTerm::Bool(operation(&left, &right))),
        _ => Err("Ordered comparison requires number operands".into()),
    }
}

fn bool_pair(
    left: &Term,
    right: &Term,
    operation: impl FnOnce(&Bool, &Bool) -> Bool,
) -> Result<ZTerm, String> {
    match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Bool(left), ZTerm::Bool(right)) => Ok(ZTerm::Bool(operation(&left, &right))),
        _ => Err("Logical operator requires boolean operands".into()),
    }
}

fn equality(left: &Term, right: &Term, negate: bool) -> Result<ZTerm, String> {
    if left == right && left.sort() == Sort::Number {
        let ZTerm::Number(value) = to_z3(left)? else {
            return Err("Equality operands have different base types".into());
        };
        let equality = value.is_nan().not();
        return Ok(ZTerm::Bool(if negate { equality.not() } else { equality }));
    }
    let equality = match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => left.eq_fpa(right),
        (ZTerm::Int(left), ZTerm::Int(right)) => left.eq(&right),
        (ZTerm::Int(left), ZTerm::Number(right)) => match int_to_number(&left)? {
            ZTerm::Number(left) => left.eq_fpa(right),
            _ => return Err("Equality operands have different base types".into()),
        },
        (ZTerm::Number(left), ZTerm::Int(right)) => match int_to_number(&right)? {
            ZTerm::Number(right) => left.eq_fpa(right),
            _ => return Err("Equality operands have different base types".into()),
        },
        (ZTerm::Bool(left), ZTerm::Bool(right)) => left.eq(&right),
        (ZTerm::String(left), ZTerm::String(right)) => left.eq(&right),
        (ZTerm::Ref(left), ZTerm::Ref(right)) => left.eq(&right),
        _ => return Err("Equality operands have different base types".into()),
    };
    Ok(ZTerm::Bool(if negate { equality.not() } else { equality }))
}

fn structural_equality(left: &Term, right: &Term) -> Result<ZTerm, String> {
    let equality = match (to_z3(left)?, to_z3(right)?) {
        (ZTerm::Number(left), ZTerm::Number(right)) => left.eq(&right),
        (ZTerm::Int(left), ZTerm::Int(right)) => left.eq(&right),
        (ZTerm::Int(left), ZTerm::Number(right)) => match int_to_number(&left)? {
            ZTerm::Number(left) => left.eq(&right),
            _ => return Err("SSA equality operands have different base types".into()),
        },
        (ZTerm::Number(left), ZTerm::Int(right)) => match int_to_number(&right)? {
            ZTerm::Number(right) => left.eq(&right),
            _ => return Err("SSA equality operands have different base types".into()),
        },
        (ZTerm::Bool(left), ZTerm::Bool(right)) => left.eq(&right),
        (ZTerm::String(left), ZTerm::String(right)) => left.eq(&right),
        (ZTerm::Ref(left), ZTerm::Ref(right)) => left.eq(&right),
        _ => return Err("SSA equality operands have different base types".into()),
    };
    Ok(ZTerm::Bool(equality))
}

fn expression_name(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.to_string()),
        Expression::StaticMemberExpression(member) => expression_name(&member.object)
            .map(|object| format!("{object}.{}", member.property.name)),
        Expression::ParenthesizedExpression(parenthesized) => {
            expression_name(&parenthesized.expression)
        }
        _ => None,
    }
}

fn expression_root_name(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.to_string()),
        Expression::StaticMemberExpression(member) => expression_root_name(&member.object),
        Expression::ParenthesizedExpression(parenthesized) => {
            expression_root_name(&parenthesized.expression)
        }
        _ => None,
    }
}

fn contains_predicate(predicate: Option<&PredicateExpr>, name: &str) -> bool {
    match predicate {
        Some(PredicateExpr::PredicateApply(found, argument)) => {
            found == name || contains_predicate(Some(argument), name)
        }
        Some(PredicateExpr::Member(object, _)) => contains_predicate(Some(object), name),
        Some(PredicateExpr::Not(inner)) => contains_predicate(Some(inner), name),
        Some(PredicateExpr::Binary(_, left, right))
        | Some(PredicateExpr::Logical(_, left, right)) => {
            contains_predicate(Some(left), name) || contains_predicate(Some(right), name)
        }
        _ => false,
    }
}

fn validate_predicate_parameter_domains(
    contract: &Contract,
    replacements: &HashMap<String, Term>,
) -> Result<(), String> {
    let mut domains = HashMap::new();
    for predicate in contract
        .params
        .iter()
        .filter_map(|(_, ty)| ty.predicate.as_ref())
        .chain(contract.ret.predicate.iter())
    {
        collect_predicate_parameter_domains(predicate, replacements, &mut domains)?;
    }
    Ok(())
}

fn collect_predicate_parameter_domains(
    predicate: &PredicateExpr,
    replacements: &HashMap<String, Term>,
    domains: &mut HashMap<String, Sort>,
) -> Result<(), String> {
    match predicate {
        PredicateExpr::PredicateApply(name, argument) => {
            let argument = predicate_term(argument, replacements, &HashMap::new(), None)?;
            let sort = argument.sort();
            if domains
                .insert(name.clone(), sort)
                .is_some_and(|found| found != sort)
            {
                return Err(format!(
                    "Predicate parameter '{name}' is applied to incompatible base types"
                ));
            }
            collect_predicate_parameter_domains(argument_expr(predicate), replacements, domains)
        }
        PredicateExpr::Not(inner) => {
            collect_predicate_parameter_domains(inner, replacements, domains)
        }
        PredicateExpr::Binary(_, left, right) | PredicateExpr::Logical(_, left, right) => {
            collect_predicate_parameter_domains(left, replacements, domains)?;
            collect_predicate_parameter_domains(right, replacements, domains)
        }
        PredicateExpr::Member(object, _) => {
            collect_predicate_parameter_domains(object, replacements, domains)
        }
        PredicateExpr::Identifier(_) | PredicateExpr::Literal(_) | PredicateExpr::Return => Ok(()),
    }
}

fn argument_expr(predicate: &PredicateExpr) -> &PredicateExpr {
    let PredicateExpr::PredicateApply(_, argument) = predicate else {
        unreachable!()
    };
    argument
}

fn sort_for_base(base: &BaseType) -> Sort {
    match base {
        BaseType::Primitive(name) if name == "boolean" => Sort::Bool,
        BaseType::Primitive(name) if name == "number" => Sort::Number,
        BaseType::Primitive(name) if name == "string" => Sort::String,
        BaseType::Union(types) => types
            .first()
            .map(sort_for_base)
            .filter(|first| types.iter().all(|ty| sort_for_base(ty) == *first))
            .unwrap_or(Sort::Ref),
        _ => Sort::Ref,
    }
}

fn contract_function_type(contract: &Contract) -> BaseType {
    BaseType::Function(
        contract
            .params
            .iter()
            .map(|(name, ty)| crate::syntax::RefinedParam {
                name: name.clone(),
                ty: ty.clone(),
            })
            .collect(),
        Box::new(contract.ret.clone()),
    )
}

fn known_member_base(base: &BaseType, property: &str) -> Option<BaseType> {
    match base {
        BaseType::Object(fields) => fields
            .iter()
            .find(|(name, _)| name == property)
            .map(|(_, field)| field.clone()),
        BaseType::Union(members) => {
            let fields = members
                .iter()
                .filter_map(|member| known_member_base(member, property))
                .collect::<Vec<_>>();
            (!fields.is_empty()).then(|| normalize_union(fields))
        }
        _ => None,
    }
}

fn known_index_base(base: &BaseType) -> Option<BaseType> {
    match base {
        BaseType::Array(element) => Some((**element).clone()),
        BaseType::Generic(name, arguments)
            if matches!(name.as_str(), "DenseArray" | "ReadonlyArray") && arguments.len() == 1 =>
        {
            arguments.first().cloned()
        }
        BaseType::Union(members) => {
            let elements = members
                .iter()
                .filter_map(known_index_base)
                .collect::<Vec<_>>();
            (!elements.is_empty()).then(|| normalize_union(elements))
        }
        _ => None,
    }
}

fn receiver_type_names(base: &BaseType) -> Vec<String> {
    match base {
        BaseType::Array(_) => vec!["Array".into()],
        BaseType::Generic(name, _) if name == "DenseArray" => {
            vec!["DenseArray".into(), "Array".into()]
        }
        BaseType::Generic(name, _) if name == "ReadonlyArray" => {
            vec!["ReadonlyArray".into(), "Array".into()]
        }
        BaseType::Generic(name, _) | BaseType::Named(name) => vec![name.clone()],
        BaseType::Primitive(name) if name == "string" => vec!["String".into()],
        BaseType::Primitive(name) if name == "number" => vec!["Number".into()],
        BaseType::Primitive(name) if name == "boolean" => vec!["Boolean".into()],
        _ => Vec::new(),
    }
}

fn catalog_property_is_intrinsic(base: &BaseType, property: &str) -> bool {
    property == "length"
        && (matches!(base, BaseType::Array(_))
            || matches!(base, BaseType::Primitive(name) if name == "string")
            || matches!(base, BaseType::Generic(name, arguments)
                if matches!(name.as_str(), "DenseArray" | "ReadonlyArray")
                    && arguments.len() == 1))
}

fn base_has_unambiguous_catalog_identity(base: &BaseType) -> bool {
    matches!(base, BaseType::Array(_))
        || matches!(
            base,
            BaseType::Primitive(name)
                if matches!(name.as_str(), "string" | "number" | "boolean")
        )
}

fn declared_base_has_catalog_identity(base: &BaseType) -> bool {
    match base {
        BaseType::Named(_) => false,
        BaseType::Generic(name, _) => {
            matches!(name.as_str(), "DenseArray" | "ReadonlyArray")
        }
        BaseType::Union(members) => {
            !members.is_empty() && members.iter().all(declared_base_has_catalog_identity)
        }
        _ => true,
    }
}

fn base_requires_catalog_identity(base: &BaseType) -> bool {
    match base {
        BaseType::Array(_) => true,
        BaseType::Generic(name, _) => {
            matches!(name.as_str(), "DenseArray" | "ReadonlyArray")
        }
        BaseType::Union(members) => members.iter().any(base_requires_catalog_identity),
        _ => false,
    }
}

fn library_base_requires_catalog_identity(base: &BaseType) -> bool {
    match base {
        BaseType::Named(name) => !name.starts_with('$'),
        BaseType::Union(members) => members.iter().any(library_base_requires_catalog_identity),
        _ => base_requires_catalog_identity(base),
    }
}

fn library_return_has_catalog_identity(base: &BaseType) -> bool {
    match base {
        BaseType::Named(name) | BaseType::Generic(name, _) => !name.starts_with('$'),
        BaseType::Union(members) => {
            !members.is_empty() && members.iter().all(library_return_has_catalog_identity)
        }
        _ => true,
    }
}

fn library_function_type(
    signature: &FunctionSignature,
    bindings: &HashMap<String, BaseType>,
) -> BaseType {
    BaseType::Function(
        signature
            .parameters
            .iter()
            .map(|parameter| crate::syntax::RefinedParam {
                name: parameter.name.clone(),
                ty: instantiate_refinement(&parameter.ty, bindings),
            })
            .collect(),
        Box::new(instantiate_refinement(&signature.returns, bindings)),
    )
}

fn normalize_union(types: Vec<BaseType>) -> BaseType {
    let mut unique = Vec::new();
    for ty in types.into_iter().flat_map(|ty| match ty {
        BaseType::Union(types) => types,
        ty => vec![ty],
    }) {
        if !unique.contains(&ty) {
            unique.push(ty);
        }
    }
    match unique.len() {
        0 => BaseType::Primitive("unknown".into()),
        1 => unique.pop().unwrap(),
        _ => BaseType::Union(unique),
    }
}

fn and_terms(terms: Vec<Term>) -> Term {
    terms
        .into_iter()
        .reduce(|left, right| Term::And(Box::new(left), Box::new(right)))
        .unwrap_or(Term::Bool(true))
}

fn intrinsic_refinements(base: &BaseType, value: &Term) -> Vec<Term> {
    let has_length = matches!(base, BaseType::Array(_))
        || matches!(base, BaseType::Primitive(name) if name == "string")
        || matches!(base, BaseType::Generic(name, arguments)
            if matches!(name.as_str(), "DenseArray" | "ReadonlyArray") && arguments.len() == 1);
    if !has_length {
        return Vec::new();
    }
    let length = collection_length(value);
    let mut refinements = vec![Term::Ge(Box::new(length.clone()), Box::new(Term::Int(0)))];
    if !matches!(base, BaseType::Primitive(name) if name == "string") {
        refinements.push(Term::Le(
            Box::new(length),
            Box::new(Term::Int(4_294_967_295)),
        ));
    }
    refinements
}

fn bases_compatible(actual: &BaseType, expected: &BaseType) -> bool {
    let mut bindings = HashMap::new();
    match_base_with_bindings(actual, expected, &mut bindings)
}

fn match_base_with_bindings(
    actual: &BaseType,
    expected: &BaseType,
    bindings: &mut HashMap<String, BaseType>,
) -> bool {
    if matches!(expected, BaseType::Primitive(name) if name == "any" || name == "unknown") {
        return true;
    }
    if let BaseType::Named(name) = expected
        && name.starts_with('$')
    {
        return match bindings.get(name) {
            Some(bound) => bases_compatible(actual, bound),
            None => {
                bindings.insert(name.clone(), actual.clone());
                true
            }
        };
    }
    match (actual, expected) {
        (BaseType::Object(_), BaseType::Primitive(name)) if name == "object" => true,
        (BaseType::Named(rendered), BaseType::Primitive(name))
            if name == "object"
                && rendered.trim_start().starts_with('{')
                && rendered.trim_end().ends_with('}') =>
        {
            true
        }
        (BaseType::Union(actual), expected) => actual
            .iter()
            .all(|actual| match_base_with_bindings(actual, expected, bindings)),
        (actual, BaseType::Union(expected)) => expected.iter().any(|expected| {
            let mut candidate = bindings.clone();
            if match_base_with_bindings(actual, expected, &mut candidate) {
                *bindings = candidate;
                true
            } else {
                false
            }
        }),
        (BaseType::Array(actual), BaseType::Array(expected)) => {
            match_mutable_array_element(actual, expected, bindings)
        }
        (BaseType::Generic(name, actual), BaseType::Array(expected))
            if name == "DenseArray" && actual.len() == 1 =>
        {
            match_mutable_array_element(&actual[0], expected, bindings)
        }
        (BaseType::Generic(actual_name, actual), BaseType::Generic(expected_name, expected))
            if actual_name == "DenseArray"
                && expected_name == "ReadonlyArray"
                && actual.len() == 1
                && expected.len() == 1 =>
        {
            match_base_with_bindings(&actual[0], &expected[0], bindings)
        }
        (BaseType::Array(actual), BaseType::Generic(name, expected))
            if name == "Iterable" && expected.len() == 1 =>
        {
            match_base_with_bindings(actual, &expected[0], bindings)
        }
        (BaseType::Generic(name, actual), BaseType::Generic(expected_name, expected))
            if matches!(name.as_str(), "DenseArray" | "ReadonlyArray")
                && expected_name == "Iterable"
                && actual.len() == 1
                && expected.len() == 1 =>
        {
            match_base_with_bindings(&actual[0], &expected[0], bindings)
        }
        (BaseType::Generic(actual_name, actual), BaseType::Generic(expected_name, expected)) => {
            actual_name == expected_name
                && actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| match_base_with_bindings(actual, expected, bindings))
        }
        (
            BaseType::Function(actual_params, actual_return),
            BaseType::Function(expected_params, expected_return),
        ) => {
            actual_params.len() <= expected_params.len()
                && actual_params
                    .iter()
                    .zip(expected_params)
                    .all(|(actual, expected)| {
                        function_parameter_refinement_is_compatible(&actual.ty, &expected.ty)
                            && match_function_parameter_with_bindings(
                                &expected.ty.base,
                                &actual.ty.base,
                                bindings,
                            )
                    })
                && function_return_refinement_is_compatible(actual_return, expected_return)
                && match_base_with_bindings(&actual_return.base, &expected_return.base, bindings)
        }
        _ => actual == expected,
    }
}

fn match_function_parameter_with_bindings(
    supplied: &BaseType,
    accepted: &BaseType,
    bindings: &mut HashMap<String, BaseType>,
) -> bool {
    if let BaseType::Named(name) = supplied
        && name.starts_with('$')
    {
        return match bindings.get(name) {
            Some(bound) => bases_compatible(bound, accepted),
            None => {
                bindings.insert(name.clone(), accepted.clone());
                true
            }
        };
    }
    match_base_with_bindings(supplied, accepted, bindings)
}

fn function_parameter_refinement_is_compatible(
    actual: &RefinementType,
    expected: &RefinementType,
) -> bool {
    actual.predicate.is_none() || actual.predicate == expected.predicate
}

fn function_return_refinement_is_compatible(
    actual: &RefinementType,
    expected: &RefinementType,
) -> bool {
    expected.predicate.is_none() || actual.predicate == expected.predicate
}

fn match_mutable_array_element(
    actual: &BaseType,
    expected: &BaseType,
    bindings: &mut HashMap<String, BaseType>,
) -> bool {
    if matches!(expected, BaseType::Named(name) if name.starts_with('$')) {
        return match_base_with_bindings(actual, expected, bindings);
    }
    actual == expected
}

fn instantiate_base(base: &BaseType, bindings: &HashMap<String, BaseType>) -> BaseType {
    match base {
        BaseType::Named(name) if name.starts_with('$') => bindings
            .get(name)
            .cloned()
            .unwrap_or_else(|| BaseType::Named(name.clone())),
        BaseType::Array(element) => BaseType::Array(Box::new(instantiate_base(element, bindings))),
        BaseType::Generic(name, arguments) => BaseType::Generic(
            name.clone(),
            arguments
                .iter()
                .map(|argument| instantiate_base(argument, bindings))
                .collect(),
        ),
        BaseType::Union(types) => BaseType::Union(
            types
                .iter()
                .map(|ty| instantiate_base(ty, bindings))
                .collect(),
        ),
        BaseType::Object(fields) => BaseType::Object(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), instantiate_base(ty, bindings)))
                .collect(),
        ),
        BaseType::Function(parameters, returns) => BaseType::Function(
            parameters
                .iter()
                .map(|parameter| crate::syntax::RefinedParam {
                    name: parameter.name.clone(),
                    ty: instantiate_refinement(&parameter.ty, bindings),
                })
                .collect(),
            Box::new(instantiate_refinement(returns, bindings)),
        ),
        other => other.clone(),
    }
}

fn instantiate_refinement(
    refinement: &RefinementType,
    bindings: &HashMap<String, BaseType>,
) -> RefinementType {
    RefinementType {
        base: instantiate_base(&refinement.base, bindings),
        index: refinement.index.clone(),
        predicate: refinement.predicate.clone(),
    }
}

fn base_for_sort(sort: Sort) -> BaseType {
    match sort {
        Sort::Number | Sort::Int => number_type(),
        Sort::Bool => boolean_type(),
        Sort::String => BaseType::Primitive("string".into()),
        Sort::Ref => BaseType::Primitive("unknown".into()),
    }
}

fn value_sort(value: &Value) -> Option<Sort> {
    match &value.base {
        BaseType::Primitive(name) if name == "number" => Some(Sort::Number),
        BaseType::Primitive(name) if name == "boolean" => Some(Sort::Bool),
        BaseType::Primitive(name) if name == "string" => Some(Sort::String),
        BaseType::Primitive(name) if name == "unknown" || name == "any" => None,
        _ => Some(Sort::Ref),
    }
}

fn is_reserved_runtime_root(name: &str) -> bool {
    matches!(name, "__rt" | "Math" | "Number" | "Array" | "console")
}

fn number_type() -> BaseType {
    BaseType::Primitive("number".into())
}

fn boolean_type() -> BaseType {
    BaseType::Primitive("boolean".into())
}

fn is_void(base: &BaseType) -> bool {
    matches!(base, BaseType::Primitive(name) if name == "void")
}

#[cfg(test)]
mod ieee_equality_tests {
    use super::*;

    #[test]
    fn number_self_equality_is_not_a_proof() {
        let x = Term::Var("x".into(), Sort::Number);
        let self_eq = Term::Eq(Box::new(x.clone()), Box::new(x.clone()));
        let is_true = Term::Eq(Box::new(self_eq), Box::new(Term::Bool(true)));
        let constraint = FixpointConstraint {
            assumptions: &[],
            consequent: &is_true,
        };
        let result = solve_constraint(&constraint).expect("solver should run");
        assert_ne!(
            result,
            SatResult::Unsat,
            "x === x must not prove true for all JS numbers (NaN)"
        );
    }
}
