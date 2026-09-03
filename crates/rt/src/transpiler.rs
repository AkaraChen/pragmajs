use crate::syntax::*;
use oxc_allocator::{Allocator, ArenaVec};
use oxc_ast::ast::*;
use oxc_ast::builder::AstBuilder;
use oxc_ast_visit::{Visit, VisitMut};
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_span::{SPAN, SourceType};
use std::collections::HashMap;

const RUNTIME_IDENT: &str = "__rt";
const RETURN_TEMP_BASE: &str = "__rt_return";
const VALUE_TEMP_BASE: &str = "__rt_v";

pub fn transpile(source: &str, annotations: &[Annotation]) -> Result<String, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::default().with_module(true);
    let mut ret = Parser::new(&allocator, source, source_type)
        .with_options(oxc_parser::ParseOptions::default())
        .parse();

    if !ret.diagnostics.is_empty() {
        return Err(format!("Parse errors: {:?}", ret.diagnostics));
    }

    let mut identifier_collector = IdentifierCollector::default();
    identifier_collector.visit_program(&ret.program);
    let mut visitor =
        TranspilerVisitor::new(&allocator, annotations, &identifier_collector.identifiers);
    visitor.visit_program(&mut ret.program);

    let code = Codegen::new().build(&ret.program).code;
    Ok(code)
}

struct TranspilerVisitor<'a> {
    allocator: &'a Allocator,
    builder: AstBuilder<'a>,
    by_function: HashMap<(String, u32), Vec<&'a Annotation>>,
    by_variable: HashMap<(String, u32), Vec<&'a Annotation>>,
    current_function: Vec<(String, u32)>,
    return_temp: String,
    value_temp: String,
}

impl<'a> TranspilerVisitor<'a> {
    fn new(
        allocator: &'a Allocator,
        annotations: &'a [Annotation],
        identifiers: &std::collections::HashSet<String>,
    ) -> Self {
        let mut by_function: HashMap<(String, u32), Vec<&Annotation>> = HashMap::new();
        let mut by_variable: HashMap<(String, u32), Vec<&Annotation>> = HashMap::new();
        for a in annotations {
            match &a.target {
                AnnotationTarget::Param {
                    function_name,
                    function_start,
                    ..
                }
                | AnnotationTarget::Return {
                    function_name,
                    function_start,
                } => {
                    by_function
                        .entry((function_name.clone(), *function_start))
                        .or_default()
                        .push(a);
                }
                AnnotationTarget::Variable {
                    name,
                    declaration_start,
                } => {
                    by_variable
                        .entry((name.clone(), *declaration_start))
                        .or_default()
                        .push(a);
                }
            }
        }
        Self {
            allocator,
            builder: AstBuilder::new(allocator),
            by_function,
            by_variable,
            current_function: Vec::new(),
            return_temp: fresh_generated_identifier(identifiers, RETURN_TEMP_BASE),
            value_temp: fresh_generated_identifier(identifiers, VALUE_TEMP_BASE),
        }
    }

    fn function_name(func: &Function) -> String {
        func.id
            .as_ref()
            .map(|id| id.name.to_string())
            .unwrap_or_else(|| "<anonymous>".into())
    }

    fn param_name(param: &FormalParameter) -> Option<String> {
        match &param.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            BindingPattern::AssignmentPattern(ap) => match &ap.left {
                BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                _ => None,
            },
            _ => None,
        }
    }

    fn alloc_str(&self, s: &str) -> &'a str {
        self.allocator.alloc_str(s)
    }

    fn ident(&self, name: &str) -> Expression<'a> {
        let name = self.alloc_str(name);
        Expression::new_identifier(SPAN, name, &self.builder)
    }

    fn string_literal(&self, value: &str) -> Expression<'a> {
        let value = self.alloc_str(value);
        Expression::new_string_literal(SPAN, value, None::<Str>, &self.builder)
    }

    fn numeric_literal(&self, value: f64) -> Expression<'a> {
        Expression::new_numeric_literal(
            SPAN,
            value,
            None::<Str>,
            NumberBase::Decimal,
            &self.builder,
        )
    }

    fn boolean_literal(&self, value: bool) -> Expression<'a> {
        Expression::new_boolean_literal(SPAN, value, &self.builder)
    }

    fn member_expr_from_expression(&self, obj: Expression<'a>, property: &str) -> Expression<'a> {
        let property = self.alloc_str(property);
        let prop = IdentifierName::new(SPAN, property, &self.builder);
        Expression::new_static_member_expression(SPAN, obj, prop, false, &self.builder)
    }

    fn member_expr(&self, object: &str, property: &str) -> Expression<'a> {
        self.member_expr_from_expression(self.ident(object), property)
    }

    fn predicate_to_expr(&self, pred: &PredicateExpr) -> Expression<'a> {
        match pred {
            PredicateExpr::Literal(lit) => match lit {
                Literal::Number(n) => self.numeric_literal(*n),
                Literal::String(s) => self.string_literal(s),
                Literal::Boolean(b) => self.boolean_literal(*b),
            },
            PredicateExpr::Identifier(name) => self.ident(name),
            PredicateExpr::Member(object, property) => {
                let object = self.predicate_to_expr(object);
                self.member_expr_from_expression(object, property)
            }
            // Predicate parameters are compile-time abstractions. Concrete
            // refinements at call sites are still checked statically.
            PredicateExpr::PredicateApply(_, _) => self.boolean_literal(true),
            PredicateExpr::Return => self.ident(&self.return_temp),
            PredicateExpr::Not(expr) => {
                let arg = self.predicate_to_expr(expr);
                Expression::new_unary_expression(
                    SPAN,
                    UnaryOperator::LogicalNot,
                    arg,
                    &self.builder,
                )
            }
            PredicateExpr::Logical(op, left, right) => {
                let op = match op {
                    LogicalOp::And => LogicalOperator::And,
                    LogicalOp::Or => LogicalOperator::Or,
                };
                let left = self.predicate_to_expr(left);
                let right = self.predicate_to_expr(right);
                Expression::new_logical_expression(SPAN, left, op, right, &self.builder)
            }
            PredicateExpr::Binary(op, left, right) => {
                let op = match op {
                    BinaryOp::EqEqEq => BinaryOperator::StrictEquality,
                    BinaryOp::NotEqEq => BinaryOperator::StrictInequality,
                    BinaryOp::EqEq => BinaryOperator::Equality,
                    BinaryOp::NotEq => BinaryOperator::Inequality,
                    BinaryOp::Gt => BinaryOperator::GreaterThan,
                    BinaryOp::Lt => BinaryOperator::LessThan,
                    BinaryOp::Gte => BinaryOperator::GreaterEqualThan,
                    BinaryOp::Lte => BinaryOperator::LessEqualThan,
                    BinaryOp::Add => BinaryOperator::Addition,
                    BinaryOp::Sub => BinaryOperator::Subtraction,
                    BinaryOp::Mul => BinaryOperator::Multiplication,
                    BinaryOp::Div => BinaryOperator::Division,
                };
                let left = self.predicate_to_expr(left);
                let right = self.predicate_to_expr(right);
                Expression::new_binary_expression(SPAN, left, op, right, &self.builder)
            }
        }
    }

    fn predicate_to_string(&self, pred: &PredicateExpr) -> String {
        match pred {
            PredicateExpr::Literal(lit) => match lit {
                Literal::Number(n) => n.to_string(),
                Literal::String(s) => format!("\"{}\"", s),
                Literal::Boolean(b) => b.to_string(),
            },
            PredicateExpr::Identifier(name) => name.clone(),
            PredicateExpr::Member(object, property) => {
                format!("{}.{}", self.predicate_to_string(object), property)
            }
            PredicateExpr::PredicateApply(name, expr) => {
                format!("{}({})", name, self.predicate_to_string(expr))
            }
            PredicateExpr::Return => "$".to_string(),
            PredicateExpr::Not(expr) => format!("!({})", self.predicate_to_string(expr)),
            PredicateExpr::Logical(op, left, right) => {
                let op_str = match op {
                    LogicalOp::And => "&&",
                    LogicalOp::Or => "||",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_string(left),
                    op_str,
                    self.predicate_to_string(right)
                )
            }
            PredicateExpr::Binary(op, left, right) => {
                let op_str = match op {
                    BinaryOp::EqEqEq => "===",
                    BinaryOp::NotEqEq => "!==",
                    BinaryOp::EqEq => "==",
                    BinaryOp::NotEq => "!=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                };
                format!(
                    "({} {} {})",
                    self.predicate_to_string(left),
                    op_str,
                    self.predicate_to_string(right)
                )
            }
        }
    }

    fn build_assert_call(
        &self,
        predicate: &PredicateExpr,
        message: &str,
        ctx_key: &str,
        ctx_value_name: &str,
    ) -> Expression<'a> {
        let ctx_key = self.alloc_str(ctx_key);
        let callee = self.member_expr(RUNTIME_IDENT, "assert");
        let cond = self.predicate_to_expr(predicate);
        let msg = self.string_literal(message);

        let ctx_key_ident =
            PropertyKey::StaticIdentifier(IdentifierName::boxed(SPAN, ctx_key, &self.builder));
        let ctx_value = self.ident(ctx_value_name);
        let prop = ObjectPropertyKind::ObjectProperty(ObjectProperty::boxed(
            SPAN,
            PropertyKind::Init,
            ctx_key_ident,
            ctx_value,
            false,
            false,
            false,
            &self.builder,
        ));
        let ctx = Expression::new_object_expression(
            SPAN,
            ArenaVec::from_array_in([prop], &self.builder),
            &self.builder,
        );

        let args = ArenaVec::from_array_in(
            [
                Argument::from(cond),
                Argument::from(msg),
                Argument::from(ctx),
            ],
            &self.builder,
        );

        Expression::new_call_expression(SPAN, callee, None, args, false, &self.builder)
    }

    fn build_param_assert(
        &self,
        function_name: &str,
        param_name: &str,
        predicate: &PredicateExpr,
    ) -> Statement<'a> {
        let msg = format!(
            "{} parameter '{}' violates refinement: {}",
            function_name,
            param_name,
            self.predicate_to_string(predicate)
        );
        let call = self.build_assert_call(predicate, &msg, param_name, param_name);
        Statement::new_expression_statement(SPAN, call, &self.builder)
    }

    fn build_return_assert(&self, function_name: &str, predicate: &PredicateExpr) -> Statement<'a> {
        let msg = format!(
            "{} return value violates refinement: {}",
            function_name,
            self.predicate_to_string(predicate)
        );
        let call = self.build_assert_call(predicate, &msg, "value", &self.return_temp);
        Statement::new_expression_statement(SPAN, call, &self.builder)
    }

    fn build_variable_assert(
        &self,
        name: &str,
        predicate: &PredicateExpr,
        value_name: &str,
    ) -> Statement<'a> {
        let renamed = rename_identifier(predicate, name, value_name);
        let msg = format!(
            "variable '{}' violates refinement: {}",
            name,
            self.predicate_to_string(predicate)
        );
        let call = self.build_assert_call(&renamed, &msg, "value", value_name);
        Statement::new_expression_statement(SPAN, call, &self.builder)
    }

    fn process_function_body(&self, func: &Function) -> ArenaVec<'a, Statement<'a>> {
        let name = Self::function_name(func);
        let key = (name.clone(), func.span.start);
        let anns = match self.by_function.get(&key) {
            Some(a) => a.clone(),
            None => return ArenaVec::new_in(&self.builder),
        };

        let mut asserts = ArenaVec::new_in(&self.builder);
        for a in &anns {
            if let AnnotationTarget::Param {
                param_name, index, ..
            } = &a.target
                && let Some(param) = func.params.items.get(*index)
                && Self::param_name(param).as_deref() == Some(param_name)
            {
                for pred in a.ty.runtime_checks() {
                    let pred = rewrite_return(&pred, param_name);
                    asserts.push(self.build_param_assert(&name, param_name, &pred));
                }
            }
        }
        asserts
    }

    fn return_predicates(&self, function: &(String, u32)) -> Vec<PredicateExpr> {
        self.by_function
            .get(function)
            .map(|anns| {
                anns.iter()
                    .filter(|a| matches!(a.target, AnnotationTarget::Return { .. }))
                    .flat_map(|a| a.ty.runtime_checks())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn build_return_iife(
        &self,
        function_name: &str,
        predicates: Vec<PredicateExpr>,
        arg: Expression<'a>,
    ) -> Expression<'a> {
        // const __rt_return = arg;
        let binding = BindingPattern::BindingIdentifier(BindingIdentifier::boxed(
            SPAN,
            self.alloc_str(&self.return_temp),
            &self.builder,
        ));
        let init_decl =
            VariableDeclarator::new(SPAN, binding, None, Some(arg), false, &self.builder);
        let mut stmts = ArenaVec::new_in(&self.builder);
        stmts.push(Statement::new_variable_declaration(
            SPAN,
            VariableDeclarationKind::Const,
            ArenaVec::from_array_in([init_decl], &self.builder),
            false,
            &self.builder,
        ));
        for pred in &predicates {
            stmts.push(self.build_return_assert(function_name, pred));
        }
        stmts.push(Statement::new_return_statement(
            SPAN,
            Some(self.ident(&self.return_temp)),
            &self.builder,
        ));

        self.wrap_in_iife(stmts)
    }

    fn build_iife_for_variable(
        &self,
        name: &str,
        predicate: &PredicateExpr,
        init: Expression<'a>,
    ) -> Expression<'a> {
        // const __rt_v = init;
        let binding = BindingPattern::BindingIdentifier(BindingIdentifier::boxed(
            SPAN,
            self.alloc_str(&self.value_temp),
            &self.builder,
        ));
        let init_decl =
            VariableDeclarator::new(SPAN, binding, None, Some(init), false, &self.builder);
        let assert_stmt = self.build_variable_assert(name, predicate, &self.value_temp);
        let return_stmt = Statement::new_return_statement(
            SPAN,
            Some(self.ident(&self.value_temp)),
            &self.builder,
        );

        let mut body_stmts = ArenaVec::new_in(&self.builder);
        body_stmts.push(Statement::new_variable_declaration(
            SPAN,
            VariableDeclarationKind::Const,
            ArenaVec::from_array_in([init_decl], &self.builder),
            false,
            &self.builder,
        ));
        body_stmts.push(assert_stmt);
        body_stmts.push(return_stmt);
        self.wrap_in_iife(body_stmts)
    }

    fn wrap_in_iife(&self, stmts: ArenaVec<'a, Statement<'a>>) -> Expression<'a> {
        let body = ArrowFunctionBody::FunctionBody(FunctionBody::boxed(
            SPAN,
            ArenaVec::new_in(&self.builder),
            stmts,
            &self.builder,
        ));
        let params = FormalParameters::boxed(
            SPAN,
            FormalParameterKind::FormalParameter,
            ArenaVec::new_in(&self.builder),
            None,
            &self.builder,
        );
        let arrow_expr = Expression::new_arrow_function_expression(
            SPAN,
            false,
            None,
            params,
            None,
            body,
            &self.builder,
        );
        Expression::new_call_expression(
            SPAN,
            arrow_expr,
            None,
            ArenaVec::new_in(&self.builder),
            false,
            &self.builder,
        )
    }
}

impl<'a> VisitMut<'a> for TranspilerVisitor<'a> {
    fn visit_function(&mut self, func: &mut Function<'a>, _flags: oxc_syntax::scope::ScopeFlags) {
        let name = Self::function_name(func);
        let key = (name, func.span.start);
        let has_function_annotations = self.by_function.contains_key(&key);

        if has_function_annotations {
            self.current_function.push(key.clone());

            let asserts = self.process_function_body(func);
            if !asserts.is_empty()
                && let Some(body) = &mut func.body
            {
                let mut new_statements = ArenaVec::new_in(&self.builder);
                new_statements.extend(asserts);
                new_statements.extend(body.statements.drain(..));
                body.statements = new_statements;
            }
        }

        oxc_ast_visit::walk_mut::walk_function(self, func, _flags);

        if has_function_annotations {
            self.current_function.pop();
        }
    }

    fn visit_return_statement(&mut self, ret: &mut ReturnStatement<'a>) {
        let function = match self.current_function.last() {
            Some(n) => n.clone(),
            None => return,
        };

        let predicates = self.return_predicates(&function);
        if predicates.is_empty() {
            return;
        }

        let arg = match ret.argument.take() {
            Some(arg) => arg,
            None => return,
        };

        ret.argument = Some(self.build_return_iife(&function.0, predicates, arg));
    }

    fn visit_variable_declaration(&mut self, decl: &mut VariableDeclaration<'a>) {
        for d in &mut decl.declarations {
            let name = match &d.id {
                BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                _ => continue,
            };
            let anns = match self.by_variable.get(&(name.clone(), d.span.start)) {
                Some(a) => a.clone(),
                None => continue,
            };
            let checks = anns
                .iter()
                .flat_map(|a| a.ty.runtime_checks())
                .collect::<Vec<_>>();
            let Some(predicate) = checks.first() else {
                continue;
            };
            let Some(init) = d.init.take() else { continue };

            d.init = Some(self.build_iife_for_variable(&name, predicate, init));
        }
    }
}

#[derive(Default)]
struct IdentifierCollector {
    identifiers: std::collections::HashSet<String>,
}

impl<'a> Visit<'a> for IdentifierCollector {
    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        self.identifiers.insert(identifier.name.to_string());
    }

    fn visit_binding_identifier(&mut self, identifier: &BindingIdentifier<'a>) {
        self.identifiers.insert(identifier.name.to_string());
    }
}

fn fresh_generated_identifier(
    identifiers: &std::collections::HashSet<String>,
    base: &str,
) -> String {
    let mut candidate = base.to_string();
    let mut suffix = 0usize;
    while identifiers.contains(&candidate) {
        suffix += 1;
        candidate = format!("{base}_{suffix}");
    }
    candidate
}

fn rewrite_return(pred: &PredicateExpr, name: &str) -> PredicateExpr {
    match pred {
        PredicateExpr::Return => PredicateExpr::Identifier(name.to_string()),
        PredicateExpr::Member(object, property) => {
            PredicateExpr::Member(Box::new(rewrite_return(object, name)), property.clone())
        }
        PredicateExpr::Not(inner) => PredicateExpr::Not(Box::new(rewrite_return(inner, name))),
        PredicateExpr::PredicateApply(pred_name, argument) => PredicateExpr::PredicateApply(
            pred_name.clone(),
            Box::new(rewrite_return(argument, name)),
        ),
        PredicateExpr::Binary(op, left, right) => PredicateExpr::Binary(
            *op,
            Box::new(rewrite_return(left, name)),
            Box::new(rewrite_return(right, name)),
        ),
        PredicateExpr::Logical(op, left, right) => PredicateExpr::Logical(
            *op,
            Box::new(rewrite_return(left, name)),
            Box::new(rewrite_return(right, name)),
        ),
        PredicateExpr::Identifier(_) | PredicateExpr::Literal(_) => pred.clone(),
    }
}

fn rename_identifier(pred: &PredicateExpr, from: &str, to: &str) -> PredicateExpr {
    match pred {
        PredicateExpr::Identifier(name) => PredicateExpr::Identifier(if name == from {
            to.to_string()
        } else {
            name.clone()
        }),
        PredicateExpr::Member(object, property) => PredicateExpr::Member(
            Box::new(rename_identifier(object, from, to)),
            property.clone(),
        ),
        PredicateExpr::PredicateApply(name, expr) => {
            PredicateExpr::PredicateApply(name.clone(), Box::new(rename_identifier(expr, from, to)))
        }
        PredicateExpr::Not(expr) => PredicateExpr::Not(Box::new(rename_identifier(expr, from, to))),
        PredicateExpr::Logical(op, left, right) => PredicateExpr::Logical(
            *op,
            Box::new(rename_identifier(left, from, to)),
            Box::new(rename_identifier(right, from, to)),
        ),
        PredicateExpr::Binary(op, left, right) => PredicateExpr::Binary(
            *op,
            Box::new(rename_identifier(left, from, to)),
            Box::new(rename_identifier(right, from, to)),
        ),
        other => other.clone(),
    }
}
