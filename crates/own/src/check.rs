//! Austral-style linearity / borrow checker over an oxc AST.

use crate::annot::{parse_own_comment, AttachedOwn, BorrowMode, FnSig, OwnDirective, OwnType};
use crate::{Diagnostic, OwnFeatures, RuleKind};
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    ArrowFunctionBody, ArrowFunctionExpression, BindingPattern, CallExpression, Expression,
    ForStatementInit, Function, FunctionBody, LogicalOperator, Statement, UnaryOperator,
    VariableDeclaration,
};
use oxc::span::GetSpan;
use oxc::syntax::operator::AssignmentOperator;
use pragma_parse::{parse, Program};
use std::collections::{HashMap, HashSet};

/// Compiler-rendered TypeScript names for omitted `/*#own` payloads.
pub trait PayloadNames {
    fn name_at(&self, byte_offset: u32) -> Option<String>;
}

impl PayloadNames for HashMap<u32, String> {
    fn name_at(&self, byte_offset: u32) -> Option<String> {
        self.get(&byte_offset).cloned()
    }
}

/// Last identifier in a compiler-rendered type (`Buffer`, `number`, …).
pub fn own_payload_name(rendered: &str) -> Option<String> {
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return None;
    }
    let stripped = rendered
        .trim_start_matches("typeof ")
        .trim()
        .split('|')
        .next()
        .unwrap_or(rendered)
        .trim()
        .split('<')
        .next()
        .unwrap_or(rendered)
        .trim();
    let name = stripped
        .rsplit(['.', ' '])
        .next()
        .unwrap_or(stripped)
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub(crate) fn check_source_with_features(
    filename: &str,
    source: &str,
    runtime: crate::Runtime,
    features: OwnFeatures,
) -> Vec<Diagnostic> {
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, source);
    check_program_with_features(filename, source, &parsed.program, runtime, features)
}

/// Check a program that has already been parsed with `pragma_parse`.
pub fn check_program(
    path: &str,
    source: &str,
    program: &Program<'_>,
    runtime: crate::Runtime,
) -> Vec<Diagnostic> {
    check_program_with_features(path, source, program, runtime, OwnFeatures::default())
}

/// Check an already parsed program with an explicit semantic feature set.
pub fn check_program_with_features(
    path: &str,
    source: &str,
    program: &Program<'_>,
    runtime: crate::Runtime,
    features: OwnFeatures,
) -> Vec<Diagnostic> {
    check_program_with_payloads_and_features(path, source, program, runtime, None, features)
}

/// Like [`check_program`], filling omitted payload names from `payloads`.
pub fn check_program_with_payloads(
    path: &str,
    source: &str,
    program: &Program<'_>,
    runtime: crate::Runtime,
    payloads: Option<&dyn PayloadNames>,
) -> Vec<Diagnostic> {
    check_program_with_payloads_and_features(
        path,
        source,
        program,
        runtime,
        payloads,
        OwnFeatures::default(),
    )
}

/// Like [`check_program_with_payloads`], with an explicit semantic feature set.
pub fn check_program_with_payloads_and_features(
    path: &str,
    source: &str,
    program: &Program<'_>,
    runtime: crate::Runtime,
    payloads: Option<&dyn PayloadNames>,
    features: OwnFeatures,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut annots: HashMap<u32, Vec<AttachedOwn>> = HashMap::new();
    for comment in program.comments.iter() {
        let content = comment.content_span().source_text(source);
        if let Some(att) = parse_own_comment(path, comment.span.start, content, &mut diags) {
            annots.entry(comment.attached_to).or_default().push(att);
        }
    }

    let prelude_sigs = crate::prelude::signatures(runtime);
    let mut file = FileCtx {
        path: path.to_string(),
        annots,
        sigs: prelude_sigs.clone(),
        scoped_sigs: HashMap::new(),
        namespace_sigs: HashMap::new(),
        collect_fn_scopes: vec![FileCtx::ROOT_SIG_SCOPE],
        collect_namespace_member: false,
        prelude_sigs,
        ns_prefix: Vec::new(),
        payloads,
        features,
    };
    file.collect_sigs_program(program);
    let mut checker = Checker {
        file: &file,
        diags,
        tbl: HashMap::new(),
        scopes: Vec::new(),
        suppress_consume: None,
        loop_depth: 0,
        fn_ret: None,
        try_finally_depth: 0,
        pending_finally: Vec::new(),
        checked_bodies: HashSet::new(),
        callee_scopes: Vec::new(),
        namespace_scopes: Vec::new(),
        features,
    };
    checker.check_program(program);
    checker.diags
}

/// Byte offsets whose omitted `/*#own` payloads should be queried from TypeScript.
pub fn omitted_payload_offsets(path: &str, source: &str, program: &Program<'_>) -> Vec<u32> {
    let mut diags = Vec::new();
    let mut annots: HashMap<u32, Vec<AttachedOwn>> = HashMap::new();
    for comment in program.comments.iter() {
        let content = comment.content_span().source_text(source);
        if let Some(att) = parse_own_comment(path, comment.span.start, content, &mut diags) {
            annots.entry(comment.attached_to).or_default().push(att);
        }
    }
    let mut offsets = Vec::new();
    collect_omitted_offsets(program, &annots, &mut offsets);
    offsets.sort();
    offsets.dedup();
    offsets
}

fn dirs_in(annots: &HashMap<u32, Vec<AttachedOwn>>, offset: u32) -> Vec<&OwnDirective> {
    let mut out = Vec::new();
    if let Some(atts) = annots.get(&offset) {
        for a in atts {
            for d in &a.directives {
                out.push(d);
            }
        }
    }
    out
}

fn type_sig_in(annots: &HashMap<u32, Vec<AttachedOwn>>, offsets: &[u32]) -> Option<FnSig> {
    for o in offsets {
        for d in dirs_in(annots, *o) {
            if let OwnDirective::Type(sig) = d {
                return Some(sig.clone());
            }
        }
    }
    None
}

fn collect_omitted_offsets(
    program: &Program<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
) {
    for stmt in &program.body {
        collect_omitted_stmt(stmt, annots, out, None);
    }
}

fn collect_omitted_stmt(
    stmt: &Statement<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: Option<u32>,
) {
    let extra = extra.into_iter().collect::<Vec<_>>();
    match stmt {
        Statement::FunctionDeclaration(f) => collect_omitted_fn(f, annots, out, &extra),
        Statement::VariableDeclaration(v) => {
            collect_omitted_var(v, annots, out, extra.first().copied())
        }
        Statement::ExportDeclaration(e) => {
            collect_omitted_stmt_decl(&e.declaration, annots, out, Some(e.span.start));
        }
        Statement::BlockStatement(b) => {
            for s in &b.body {
                collect_omitted_stmt(s, annots, out, None);
            }
        }
        _ => {}
    }
}

fn collect_omitted_stmt_decl(
    decl: &oxc::ast::ast::Declaration<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: Option<u32>,
) {
    match decl {
        oxc::ast::ast::Declaration::FunctionDeclaration(f) => {
            let extra = extra.into_iter().collect::<Vec<_>>();
            collect_omitted_fn(f, annots, out, &extra)
        }
        oxc::ast::ast::Declaration::VariableDeclaration(v) => {
            collect_omitted_var(v, annots, out, extra)
        }
        _ => {}
    }
}

fn collect_omitted_fn(
    func: &Function<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: &[u32],
) {
    let mut offs = extra.to_vec();
    offs.push(func.span.start);
    if let Some(id) = &func.id {
        offs.push(id.span.start);
    }
    if let Some(sig) = type_sig_in(annots, &offs) {
        for (i, (_, ty)) in sig.params.iter().enumerate() {
            if ty.payload_omitted() {
                if let Some(p) = func.params.items.get(i) {
                    out.push(p.span.start);
                }
            }
        }
        if sig.ret.payload_omitted() {
            out.push(func.span.start);
        }
    }
    if let Some(body) = &func.body {
        for s in &body.statements {
            collect_omitted_stmt(s, annots, out, None);
        }
    }
}

fn collect_omitted_arrow(
    arrow: &ArrowFunctionExpression<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: &[u32],
) {
    let mut offs = extra.to_vec();
    offs.push(arrow.span.start);
    if let Some(sig) = type_sig_in(annots, &offs) {
        for (i, (_, ty)) in sig.params.iter().enumerate() {
            if ty.payload_omitted() {
                if let Some(p) = arrow.params.items.get(i) {
                    out.push(p.span.start);
                }
            }
        }
        if sig.ret.payload_omitted() {
            out.push(arrow.span.start);
        }
    }
}

fn collect_omitted_init(
    init: &Expression<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: &[u32],
) {
    match peel(init) {
        Expression::FunctionExpression(func) => collect_omitted_fn(func, annots, out, extra),
        Expression::ArrowFunctionExpression(arrow) => {
            collect_omitted_arrow(arrow, annots, out, extra)
        }
        Expression::AssignmentExpression(assign) => {
            collect_omitted_init(&assign.right, annots, out, extra)
        }
        Expression::SequenceExpression(seq) => {
            if let Some(expr) = seq.expressions.last() {
                collect_omitted_init(expr, annots, out, extra);
            }
        }
        Expression::LogicalExpression(logical) => {
            collect_omitted_init(&logical.left, annots, out, extra);
            collect_omitted_init(&logical.right, annots, out, extra);
        }
        Expression::ConditionalExpression(cond) => {
            collect_omitted_init(&cond.consequent, annots, out, extra);
            collect_omitted_init(&cond.alternate, annots, out, extra);
        }
        _ => {}
    }
}

fn collect_omitted_var(
    decl: &VariableDeclaration<'_>,
    annots: &HashMap<u32, Vec<AttachedOwn>>,
    out: &mut Vec<u32>,
    extra: Option<u32>,
) {
    for d in &decl.declarations {
        let mut dirs: Vec<&OwnDirective> = dirs_in(annots, decl.span.start);
        dirs.extend(dirs_in(annots, d.span.start));
        let omitted_binding = dirs.iter().any(|dir| match dir {
            OwnDirective::Let { ty, .. } | OwnDirective::Kind(ty) => ty.payload_omitted(),
            _ => false,
        });
        if omitted_binding {
            out.push(d.span.start);
            if let Some(id) = ident_span(&d.id) {
                out.push(id);
            }
        }
        let mut offs = vec![decl.span.start, d.span.start];
        if let Some(e) = extra {
            offs.push(e);
        }
        if let Some(init) = &d.init {
            collect_omitted_init(init, annots, out, &offs);
        }
    }
}

fn ident_span(pat: &BindingPattern<'_>) -> Option<u32> {
    match pat {
        BindingPattern::BindingIdentifier(id) => Some(id.span.start),
        _ => None,
    }
}

struct FileCtx<'a> {
    path: String,
    annots: HashMap<u32, Vec<AttachedOwn>>,
    sigs: HashMap<String, FnSig>,
    scoped_sigs: HashMap<u32, HashMap<String, FnSig>>,
    namespace_sigs: HashMap<String, HashMap<String, FnSig>>,
    collect_fn_scopes: Vec<u32>,
    collect_namespace_member: bool,
    prelude_sigs: HashMap<String, FnSig>,
    ns_prefix: Vec<String>,
    payloads: Option<&'a dyn PayloadNames>,
    features: OwnFeatures,
}

impl FileCtx<'_> {
    const ROOT_SIG_SCOPE: u32 = u32::MAX;

    fn insert_sig(&mut self, name: String, sig: FnSig) {
        if name.contains('.') || name.contains('#') {
            self.sigs.insert(name, sig);
            return;
        }
        let scope = self
            .collect_fn_scopes
            .last()
            .copied()
            .unwrap_or(Self::ROOT_SIG_SCOPE);
        self.scoped_sigs.entry(scope).or_default().insert(name, sig);
    }

    fn insert_decl_sig(&mut self, name: String, sig: FnSig, is_namespace_member: bool) {
        if !is_namespace_member {
            self.insert_sig(name, sig);
            return;
        }

        let namespace = self.ns_prefix.join(".");
        self.namespace_sigs
            .entry(namespace.clone())
            .or_default()
            .insert(name.clone(), sig.clone());
        self.sigs.insert(format!("{namespace}.{name}"), sig);
    }

    fn dirs_at(&self, offset: u32) -> Vec<&OwnDirective> {
        let mut out = Vec::new();
        if let Some(atts) = self.annots.get(&offset) {
            for a in atts {
                for d in &a.directives {
                    out.push(d);
                }
            }
        }
        out
    }

    fn type_sig_at(&self, offsets: &[u32]) -> Option<FnSig> {
        if !self.features.function_contracts {
            return None;
        }
        for o in offsets {
            for d in self.dirs_at(*o) {
                if let OwnDirective::Type(sig) = d {
                    return Some(sig.clone());
                }
            }
        }
        None
    }

    fn collect_sigs_program(&mut self, program: &Program<'_>) {
        for stmt in &program.body {
            self.collect_sigs_stmt(stmt, None);
        }
    }

    fn collect_sigs_stmt(&mut self, stmt: &Statement<'_>, extra: Option<u32>) {
        match stmt {
            Statement::FunctionDeclaration(f) => self.collect_fn(f, extra),
            Statement::ClassDeclaration(c) => self.collect_class(c, None),
            Statement::VariableDeclaration(v) => self.collect_var(v, extra),
            Statement::ExportDeclaration(e) => {
                self.collect_sigs_decl(&e.declaration, Some(e.span.start));
            }
            Statement::ExportDefaultDeclaration(e) => {
                self.collect_sigs_export_default(e);
            }
            Statement::BlockStatement(b) => {
                for s in &b.body {
                    self.collect_sigs_stmt(s, None);
                }
            }
            Statement::IfStatement(i) => {
                self.collect_sigs_expr(&i.test);
                self.collect_sigs_stmt(&i.consequent, None);
                if let Some(a) = &i.alternate {
                    self.collect_sigs_stmt(a, None);
                }
            }
            Statement::WhileStatement(w) => {
                self.collect_sigs_expr(&w.test);
                self.collect_sigs_stmt(&w.body, None);
            }
            Statement::DoWhileStatement(w) => {
                self.collect_sigs_stmt(&w.body, None);
                self.collect_sigs_expr(&w.test);
            }
            Statement::ForStatement(f) => {
                if let Some(init) = &f.init {
                    match init {
                        ForStatementInit::VariableDeclaration(v) => self.collect_var(v, None),
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_sigs_expr(e);
                            }
                        }
                    }
                }
                if let Some(test) = &f.test {
                    self.collect_sigs_expr(test);
                }
                if let Some(upd) = &f.update {
                    self.collect_sigs_expr(upd);
                }
                self.collect_sigs_stmt(&f.body, None);
            }
            Statement::ForInStatement(f) => {
                self.collect_sigs_expr(&f.right);
                match &f.left {
                    oxc::ast::ast::ForStatementLeft::VariableDeclaration(v) => {
                        self.collect_var(v, None);
                    }
                    other => {
                        if let Some(t) = other.as_assignment_target() {
                            self.collect_sigs_assignment_target(t);
                            self.collect_assignment_defaults(t);
                        }
                    }
                }
                self.collect_sigs_stmt(&f.body, None);
            }
            Statement::ForOfStatement(f) => {
                self.collect_sigs_expr(&f.right);
                self.collect_for_left_object_methods(&f.left, &f.right);
                self.collect_sigs_stmt(&f.body, None);
            }
            Statement::SwitchStatement(s) => {
                self.collect_sigs_expr(&s.discriminant);
                for c in &s.cases {
                    if let Some(test) = &c.test {
                        self.collect_sigs_expr(test);
                    }
                    for st in &c.consequent {
                        self.collect_sigs_stmt(st, None);
                    }
                }
            }
            Statement::ReturnStatement(r) => {
                if let Some(a) = &r.argument {
                    self.collect_sigs_expr(a);
                }
            }
            Statement::ThrowStatement(t) => self.collect_sigs_expr(&t.argument),
            Statement::TryStatement(t) => {
                for s in &t.block.body {
                    self.collect_sigs_stmt(s, None);
                }
                if let Some(h) = &t.handler {
                    if let Some(p) = &h.param {
                        self.collect_binding_defaults(&p.pattern);
                    }
                    for s in &h.body.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
                if let Some(f) = &t.finalizer {
                    for s in &f.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
            }
            Statement::LabeledStatement(l) => self.collect_sigs_stmt(&l.body, None),
            Statement::ExpressionStatement(e) => self.collect_sigs_expr(&e.expression),
            Statement::WithStatement(w) => {
                self.collect_sigs_expr(&w.object);
                self.collect_sigs_stmt(&w.body, None);
            }
            Statement::TSNamespaceDeclaration(n) => self.collect_sigs_namespace(n),
            Statement::TSGlobalDeclaration(g) => {
                for s in &g.body.body {
                    self.collect_sigs_stmt(s, None);
                }
            }
            Statement::TSExternalModuleDeclaration(m) => {
                if let Some(b) = &m.body {
                    for s in &b.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
            }
            Statement::TSEnumDeclaration(e) => self.collect_sigs_enum(e),
            Statement::TSInterfaceDeclaration(i) => self.collect_sigs_interface(i),
            Statement::TSTypeAliasDeclaration(t) => self.collect_sigs_type_alias(t),
            Statement::TSExportAssignment(e) => self.collect_sigs_expr(&e.expression),
            _ => {}
        }
    }

    fn collect_sigs_namespace(&mut self, n: &oxc::ast::ast::TSNamespaceDeclaration<'_>) {
        self.ns_prefix.push(n.id.name.as_str().to_string());
        match &n.body {
            oxc::ast::ast::TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
                self.collect_sigs_namespace(inner);
            }
            oxc::ast::ast::TSNamespaceDeclarationBody::TSModuleBlock(b) => {
                for s in &b.body {
                    self.collect_sigs_namespace_member(s);
                }
            }
        }
        self.ns_prefix.pop();
    }

    fn collect_sigs_namespace_member(&mut self, stmt: &Statement<'_>) {
        let is_member = matches!(
            stmt,
            Statement::FunctionDeclaration(_) | Statement::VariableDeclaration(_)
        ) || matches!(
            stmt,
            Statement::ExportDeclaration(export)
                if matches!(
                    export.declaration,
                    oxc::ast::ast::Declaration::FunctionDeclaration(_)
                        | oxc::ast::ast::Declaration::VariableDeclaration(_)
                )
        );
        let saved = std::mem::replace(&mut self.collect_namespace_member, is_member);
        self.collect_sigs_stmt(stmt, None);
        self.collect_namespace_member = saved;
    }

    fn collect_sigs_decl(&mut self, decl: &oxc::ast::ast::Declaration<'_>, extra: Option<u32>) {
        match decl {
            oxc::ast::ast::Declaration::FunctionDeclaration(f) => self.collect_fn(f, extra),
            oxc::ast::ast::Declaration::ClassDeclaration(c) => self.collect_class(c, None),
            oxc::ast::ast::Declaration::VariableDeclaration(v) => self.collect_var(v, extra),
            oxc::ast::ast::Declaration::TSNamespaceDeclaration(n) => self.collect_sigs_namespace(n),
            oxc::ast::ast::Declaration::TSGlobalDeclaration(g) => {
                for s in &g.body.body {
                    self.collect_sigs_stmt(s, None);
                }
            }
            oxc::ast::ast::Declaration::TSExternalModuleDeclaration(m) => {
                if let Some(b) = &m.body {
                    for s in &b.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
            }
            oxc::ast::ast::Declaration::TSEnumDeclaration(e) => self.collect_sigs_enum(e),
            oxc::ast::ast::Declaration::TSInterfaceDeclaration(i) => self.collect_sigs_interface(i),
            oxc::ast::ast::Declaration::TSTypeAliasDeclaration(t) => {
                self.collect_sigs_type_alias(t)
            }
            _ => {}
        }
    }

    fn collect_sigs_export_default(&mut self, e: &oxc::ast::ast::ExportDefaultDeclaration<'_>) {
        match &e.declaration {
            oxc::ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f)
            | oxc::ast::ast::ExportDefaultDeclarationKind::FunctionExpression(f) => {
                self.collect_fn(f, Some(e.span.start));
            }
            oxc::ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(c)
            | oxc::ast::ast::ExportDefaultDeclarationKind::ClassExpression(c) => {
                self.collect_class(c, None);
            }
            other => {
                if let Some(expr) = other.as_expression() {
                    self.collect_sigs_expr(expr);
                }
            }
        }
    }

    fn collect_fn(&mut self, func: &Function<'_>, extra: Option<u32>) {
        let is_namespace_member = std::mem::take(&mut self.collect_namespace_member);
        let mut offs = vec![func.span.start];
        if let Some(e) = extra {
            offs.push(e);
        }
        if let Some(id) = &func.id {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            if let Some(name) = func.name() {
                self.insert_decl_sig(name.as_str().to_string(), sig, is_namespace_member);
            }
        }
        self.collect_fn_scopes.push(func.span.start);
        if let Some(tp) = &func.type_parameters {
            self.collect_ts_type_params(tp);
        }
        if let Some(this) = &func.this_param {
            if let Some(t) = &this.type_annotation {
                self.collect_ts_ann(t);
            }
        }
        if let Some(rt) = &func.return_type {
            self.collect_ts_ann(rt);
        }
        for p in &func.params.items {
            self.collect_sigs_decorators(&p.decorators);
            if let Some(t) = &p.type_annotation {
                self.collect_ts_ann(t);
            }
        }
        self.collect_param_default_object_methods(&func.params);
        if let Some(body) = &func.body {
            for s in &body.statements {
                self.collect_sigs_stmt(s, None);
            }
        }
        self.collect_fn_scopes.pop();
    }

    fn collect_var(&mut self, decl: &VariableDeclaration<'_>, extra: Option<u32>) {
        let is_namespace_member = std::mem::take(&mut self.collect_namespace_member);
        for d in &decl.declarations {
            if let Some(t) = &d.type_annotation {
                self.collect_ts_ann(t);
            }
            let mut offs = vec![decl.span.start, d.span.start];
            if let Some(e) = extra {
                offs.push(e);
            }
            if let Some(init) = &d.init {
                collect_fn_init_offs(init, &mut offs);
            }
            if let Some(sig) = self.type_sig_at(&offs) {
                if let Some(name) = ident_of_pattern(&d.id) {
                    self.insert_decl_sig(name, sig, is_namespace_member);
                }
            }
            self.collect_binding_defaults(&d.id);
            if let Some(init) = &d.init {
                self.collect_sigs_expr(init);
                self.collect_binding_object_methods(&d.id, init);
            }
        }
    }

    fn collect_sigs_expr(&mut self, expr: &Expression<'_>) {
        self.collect_type_wrappers(expr);
        match peel(expr) {
            Expression::FunctionExpression(f) => self.collect_fn(f, None),
            Expression::ArrowFunctionExpression(a) => self.collect_sigs_arrow(a),
            Expression::ClassExpression(c) => self.collect_class(c, None),
            Expression::ObjectExpression(o) => self.collect_object(o),
            Expression::CallExpression(c) => {
                if let Some(ta) = &c.type_arguments {
                    self.collect_ts_type_args(ta);
                }
                self.collect_sigs_expr(&c.callee);
                for a in &c.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.collect_sigs_expr(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_sigs_expr(e);
                            }
                        }
                    }
                }
            }
            Expression::NewExpression(n) => {
                if let Some(ta) = &n.type_arguments {
                    self.collect_ts_type_args(ta);
                }
                self.collect_sigs_expr(&n.callee);
                for a in &n.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.collect_sigs_expr(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_sigs_expr(e);
                            }
                        }
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.collect_sigs_expr(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_sigs_expr(e);
                            }
                        }
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.collect_sigs_expr(e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_sigs_expr(&b.left);
                self.collect_sigs_expr(&b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_sigs_expr(&c.test);
                self.collect_sigs_expr(&c.consequent);
                self.collect_sigs_expr(&c.alternate);
            }
            Expression::UnaryExpression(u) => self.collect_sigs_expr(&u.argument),
            Expression::BinaryExpression(b) => {
                self.collect_sigs_expr(&b.left);
                self.collect_sigs_expr(&b.right);
            }
            Expression::AssignmentExpression(a) => {
                self.collect_sigs_assignment_target(&a.left);
                self.collect_sigs_expr(&a.right);
                self.collect_assignment_object_methods(&a.left, &a.right);
            }
            Expression::UpdateExpression(u) => match &u.argument {
                oxc::ast::ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                    self.collect_sigs_expr(&m.object);
                    self.collect_sigs_expr(&m.expression);
                }
                oxc::ast::ast::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                    self.collect_sigs_expr(&m.object)
                }
                oxc::ast::ast::SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                    self.collect_sigs_expr(&m.object)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSAsExpression(e) => {
                    self.collect_sigs_expr(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => {
                    self.collect_sigs_expr(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSNonNullExpression(e) => {
                    self.collect_sigs_expr(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSTypeAssertion(e) => {
                    self.collect_sigs_expr(&e.expression)
                }
                _ => {}
            },
            Expression::StaticMemberExpression(m) => self.collect_sigs_expr(&m.object),
            Expression::ComputedMemberExpression(m) => {
                self.collect_sigs_expr(&m.object);
                self.collect_sigs_expr(&m.expression);
            }
            Expression::PrivateFieldExpression(m) => self.collect_sigs_expr(&m.object),
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.collect_sigs_expr(e);
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                if let Some(ta) = &t.type_arguments {
                    self.collect_ts_type_args(ta);
                }
                self.collect_sigs_expr(&t.tag);
                for e in &t.quasi.expressions {
                    self.collect_sigs_expr(e);
                }
            }
            Expression::YieldExpression(y) => {
                if let Some(a) = &y.argument {
                    self.collect_sigs_expr(a);
                }
            }
            Expression::ImportExpression(i) => {
                self.collect_sigs_expr(&i.source);
                if let Some(o) = &i.options {
                    self.collect_sigs_expr(o);
                }
            }
            Expression::V8IntrinsicExpression(v) => {
                for a in &v.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_sigs_expr(e);
                    }
                }
            }
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => {
                    if let Some(ta) = &call.type_arguments {
                        self.collect_ts_type_args(ta);
                    }
                    self.collect_sigs_expr(&call.callee);
                    for a in &call.arguments {
                        match a {
                            oxc::ast::ast::Argument::SpreadElement(s) => {
                                self.collect_sigs_expr(&s.argument)
                            }
                            other => {
                                if let Some(e) = other.as_expression() {
                                    self.collect_sigs_expr(e);
                                }
                            }
                        }
                    }
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    self.collect_sigs_expr(&m.object)
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.collect_sigs_expr(&m.object);
                    self.collect_sigs_expr(&m.expression);
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    self.collect_sigs_expr(&m.object)
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.collect_sigs_expr(&n.expression)
                }
            },
            Expression::PrivateInExpression(p) => self.collect_sigs_expr(&p.right),
            Expression::JSXElement(el) => {
                for attr in &el.opening_element.attributes {
                    match attr {
                        oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                            self.collect_sigs_expr(&s.argument)
                        }
                        oxc::ast::ast::JSXAttributeItem::Attribute(a) => {
                            if let Some(oxc::ast::ast::JSXAttributeValue::ExpressionContainer(e)) =
                                &a.value
                            {
                                if let Some(x) = e.expression.as_expression() {
                                    self.collect_sigs_expr(x);
                                }
                            }
                        }
                    }
                }
                for c in &el.children {
                    self.collect_sigs_jsx_child(c);
                }
            }
            Expression::JSXFragment(f) => {
                for c in &f.children {
                    self.collect_sigs_jsx_child(c);
                }
            }
            _ => {}
        }
    }

    fn collect_sigs_jsx_child(&mut self, c: &oxc::ast::ast::JSXChild<'_>) {
        match c {
            oxc::ast::ast::JSXChild::Element(e) => {
                for attr in &e.opening_element.attributes {
                    match attr {
                        oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                            self.collect_sigs_expr(&s.argument)
                        }
                        oxc::ast::ast::JSXAttributeItem::Attribute(a) => {
                            if let Some(oxc::ast::ast::JSXAttributeValue::ExpressionContainer(x)) =
                                &a.value
                            {
                                if let Some(expr) = x.expression.as_expression() {
                                    self.collect_sigs_expr(expr);
                                }
                            }
                        }
                    }
                }
                for ch in &e.children {
                    self.collect_sigs_jsx_child(ch);
                }
            }
            oxc::ast::ast::JSXChild::Fragment(f) => {
                for ch in &f.children {
                    self.collect_sigs_jsx_child(ch);
                }
            }
            oxc::ast::ast::JSXChild::ExpressionContainer(e) => {
                if let Some(x) = e.expression.as_expression() {
                    self.collect_sigs_expr(x);
                }
            }
            oxc::ast::ast::JSXChild::Spread(s) => self.collect_sigs_expr(&s.expression),
            oxc::ast::ast::JSXChild::Text(_) => {}
        }
    }

    fn collect_class(&mut self, class: &oxc::ast::ast::Class<'_>, bind_name: Option<&str>) {
        self.collect_sigs_decorators(&class.decorators);
        if let Some(tp) = &class.type_parameters {
            self.collect_ts_type_params(tp);
        }
        if let Some(h) = &class.heritage {
            self.collect_sigs_expr(&h.expression);
            if let Some(ta) = &h.type_arguments {
                self.collect_ts_type_args(ta);
            }
        }
        for impls in &class.implements {
            if let Some(ta) = &impls.type_arguments {
                self.collect_ts_type_args(ta);
            }
        }
        let mut names = Vec::new();
        if let Some(id) = &class.id {
            names.push(id.name.as_str().to_string());
        }
        if let Some(b) = bind_name {
            if !names.iter().any(|n| n == b) {
                names.push(b.to_string());
            }
        }
        for el in &class.body.body {
            match el {
                oxc::ast::ast::ClassElement::MethodDefinition(m) => {
                    self.collect_sigs_decorators(&m.decorators);
                    if let Some(k) = m.key.as_expression() {
                        self.collect_sigs_expr(k);
                    }
                    self.collect_fn(&m.value, Some(m.span.start));
                    for name in &names {
                        self.collect_class_named_sig(
                            Some(name),
                            m.r#static,
                            &m.key,
                            m.span.start,
                            m.value.span.start,
                        );
                    }
                }
                oxc::ast::ast::ClassElement::PropertyDefinition(p) => {
                    self.collect_sigs_decorators(&p.decorators);
                    if let Some(k) = p.key.as_expression() {
                        self.collect_sigs_expr(k);
                    }
                    if let Some(t) = &p.type_annotation {
                        self.collect_ts_ann(t);
                    }
                    if let Some(v) = &p.value {
                        self.collect_sigs_expr(v);
                        for name in &names {
                            self.collect_class_named_sig(
                                Some(name),
                                p.r#static,
                                &p.key,
                                p.span.start,
                                v.span().start,
                            );
                        }
                    }
                }
                oxc::ast::ast::ClassElement::AccessorProperty(p) => {
                    self.collect_sigs_decorators(&p.decorators);
                    if let Some(k) = p.key.as_expression() {
                        self.collect_sigs_expr(k);
                    }
                    if let Some(t) = &p.type_annotation {
                        self.collect_ts_ann(t);
                    }
                    if let Some(v) = &p.value {
                        self.collect_sigs_expr(v);
                        for name in &names {
                            self.collect_class_named_sig(
                                Some(name),
                                p.r#static,
                                &p.key,
                                p.span.start,
                                v.span().start,
                            );
                        }
                    }
                }
                oxc::ast::ast::ClassElement::StaticBlock(b) => {
                    for s in &b.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
                oxc::ast::ast::ClassElement::TSIndexSignature(i) => {
                    self.collect_ts_ann(&i.parameter.type_annotation);
                    self.collect_ts_ann(&i.type_annotation);
                }
            }
        }
    }

    fn collect_sigs_decorators(&mut self, decs: &[oxc::ast::ast::Decorator<'_>]) {
        for d in decs {
            self.collect_sigs_expr(&d.expression);
        }
    }

    fn collect_sigs_enum(&mut self, e: &oxc::ast::ast::TSEnumDeclaration<'_>) {
        for m in &e.body.members {
            if let oxc::ast::ast::TSEnumMemberName::ComputedTemplateString(t) = &m.id {
                for expr in &t.expressions {
                    self.collect_sigs_expr(expr);
                }
            }
            if let Some(init) = &m.initializer {
                self.collect_sigs_expr(init);
            }
        }
    }

    fn collect_sigs_interface(&mut self, i: &oxc::ast::ast::TSInterfaceDeclaration<'_>) {
        if let Some(tp) = &i.type_parameters {
            self.collect_ts_type_params(tp);
        }
        for e in &i.extends {
            if let Some(ta) = &e.type_arguments {
                self.collect_ts_type_args(ta);
            }
        }
        for s in &i.body.body {
            self.collect_ts_signature(s);
        }
    }

    fn collect_sigs_type_alias(&mut self, t: &oxc::ast::ast::TSTypeAliasDeclaration<'_>) {
        if let Some(tp) = &t.type_parameters {
            self.collect_ts_type_params(tp);
        }
        self.collect_ts_type(&t.type_annotation);
    }

    fn collect_type_wrappers(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::ParenthesizedExpression(p) => self.collect_type_wrappers(&p.expression),
            Expression::AwaitExpression(a) => self.collect_type_wrappers(&a.argument),
            Expression::TSAsExpression(e) => {
                self.collect_ts_type(&e.type_annotation);
                self.collect_type_wrappers(&e.expression);
            }
            Expression::TSSatisfiesExpression(e) => {
                self.collect_ts_type(&e.type_annotation);
                self.collect_type_wrappers(&e.expression);
            }
            Expression::TSTypeAssertion(e) => {
                self.collect_ts_type(&e.type_annotation);
                self.collect_type_wrappers(&e.expression);
            }
            Expression::TSNonNullExpression(e) => self.collect_type_wrappers(&e.expression),
            Expression::TSInstantiationExpression(e) => {
                self.collect_ts_type_args(&e.type_arguments);
                self.collect_type_wrappers(&e.expression);
            }
            _ => {}
        }
    }

    fn collect_ts_ann(&mut self, ann: &oxc::ast::ast::TSTypeAnnotation<'_>) {
        self.collect_ts_type(&ann.type_annotation);
    }

    fn collect_ts_type_args(&mut self, ta: &oxc::ast::ast::TSTypeParameterInstantiation<'_>) {
        for t in &ta.params {
            self.collect_ts_type(t);
        }
    }

    fn collect_ts_type_params(&mut self, tp: &oxc::ast::ast::TSTypeParameterDeclaration<'_>) {
        for p in &tp.params {
            if let Some(c) = &p.constraint {
                self.collect_ts_type(c);
            }
            if let Some(d) = &p.default {
                self.collect_ts_type(d);
            }
        }
    }

    fn collect_ts_type(&mut self, ty: &oxc::ast::ast::TSType<'_>) {
        match ty {
            oxc::ast::ast::TSType::TSTypeLiteral(l) => {
                for m in &l.members {
                    self.collect_ts_signature(m);
                }
            }
            oxc::ast::ast::TSType::TSArrayType(a) => self.collect_ts_type(&a.element_type),
            oxc::ast::ast::TSType::TSUnionType(u) => {
                for t in &u.types {
                    self.collect_ts_type(t);
                }
            }
            oxc::ast::ast::TSType::TSIntersectionType(i) => {
                for t in &i.types {
                    self.collect_ts_type(t);
                }
            }
            oxc::ast::ast::TSType::TSParenthesizedType(p) => {
                self.collect_ts_type(&p.type_annotation)
            }
            oxc::ast::ast::TSType::TSTypeOperatorType(o) => {
                self.collect_ts_type(&o.type_annotation)
            }
            oxc::ast::ast::TSType::TSIndexedAccessType(i) => {
                self.collect_ts_type(&i.object_type);
                self.collect_ts_type(&i.index_type);
            }
            oxc::ast::ast::TSType::TSConditionalType(c) => {
                self.collect_ts_type(&c.check_type);
                self.collect_ts_type(&c.extends_type);
                self.collect_ts_type(&c.true_type);
                self.collect_ts_type(&c.false_type);
            }
            oxc::ast::ast::TSType::TSTupleType(t) => {
                for el in &t.element_types {
                    if let Some(inner) = el.as_ts_type() {
                        self.collect_ts_type(inner);
                    } else {
                        match el {
                            oxc::ast::ast::TSTupleElement::TSOptionalType(o) => {
                                self.collect_ts_type(&o.type_annotation);
                            }
                            oxc::ast::ast::TSTupleElement::TSRestType(r) => {
                                self.collect_ts_type(&r.type_annotation);
                            }
                            _ => {}
                        }
                    }
                }
            }
            oxc::ast::ast::TSType::TSFunctionType(f) => {
                if let Some(tp) = &f.type_parameters {
                    self.collect_ts_type_params(tp);
                }
                if let Some(this) = &f.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.collect_ts_ann(t);
                    }
                }
                self.collect_ts_params(&f.params);
                self.collect_ts_ann(&f.return_type);
            }
            oxc::ast::ast::TSType::TSConstructorType(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.collect_ts_type_params(tp);
                }
                self.collect_ts_params(&c.params);
                self.collect_ts_ann(&c.return_type);
            }
            oxc::ast::ast::TSType::TSInferType(i) => {
                if let Some(c) = &i.type_parameter.constraint {
                    self.collect_ts_type(c);
                }
                if let Some(d) = &i.type_parameter.default {
                    self.collect_ts_type(d);
                }
            }
            oxc::ast::ast::TSType::TSTemplateLiteralType(t) => {
                for inner in &t.types {
                    self.collect_ts_type(inner);
                }
            }
            oxc::ast::ast::TSType::TSTypeReference(r) => {
                if let Some(ta) = &r.type_arguments {
                    self.collect_ts_type_args(ta);
                }
            }
            oxc::ast::ast::TSType::TSMappedType(m) => {
                self.collect_ts_type(&m.constraint);
                if let Some(n) = &m.name_type {
                    self.collect_ts_type(n);
                }
                if let Some(a) = &m.type_annotation {
                    self.collect_ts_type(a);
                }
            }
            oxc::ast::ast::TSType::TSNamedTupleMember(m) => {
                if let Some(inner) = m.element_type.as_ts_type() {
                    self.collect_ts_type(inner);
                }
            }
            oxc::ast::ast::TSType::TSTypePredicate(p) => {
                if let Some(t) = &p.type_annotation {
                    self.collect_ts_ann(t);
                }
            }
            oxc::ast::ast::TSType::TSImportType(i) => {
                if let Some(o) = &i.options {
                    self.collect_object(o);
                }
                if let Some(ta) = &i.type_arguments {
                    self.collect_ts_type_args(ta);
                }
            }
            oxc::ast::ast::TSType::TSTypeQuery(q) => {
                if let oxc::ast::ast::TSTypeQueryExprName::TSImportType(i) = &q.expr_name {
                    if let Some(o) = &i.options {
                        self.collect_object(o);
                    }
                    if let Some(ta) = &i.type_arguments {
                        self.collect_ts_type_args(ta);
                    }
                }
                if let Some(ta) = &q.type_arguments {
                    self.collect_ts_type_args(ta);
                }
            }
            oxc::ast::ast::TSType::JSDocNullableType(t) => self.collect_ts_type(&t.type_annotation),
            oxc::ast::ast::TSType::JSDocNonNullableType(t) => {
                self.collect_ts_type(&t.type_annotation)
            }
            _ => {}
        }
    }

    fn collect_ts_params(&mut self, params: &oxc::ast::ast::FormalParameters<'_>) {
        for p in &params.items {
            if let Some(t) = &p.type_annotation {
                self.collect_ts_ann(t);
            }
        }
        if let Some(r) = &params.rest {
            if let Some(t) = &r.type_annotation {
                self.collect_ts_ann(t);
            }
        }
    }

    fn collect_ts_signature(&mut self, s: &oxc::ast::ast::TSSignature<'_>) {
        match s {
            oxc::ast::ast::TSSignature::TSPropertySignature(p) => {
                if let Some(k) = p.key.as_expression() {
                    self.collect_sigs_expr(k);
                }
                if let Some(t) = &p.type_annotation {
                    self.collect_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSMethodSignature(m) => {
                if let Some(k) = m.key.as_expression() {
                    self.collect_sigs_expr(k);
                }
                if let Some(tp) = &m.type_parameters {
                    self.collect_ts_type_params(tp);
                }
                if let Some(this) = &m.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.collect_ts_ann(t);
                    }
                }
                self.collect_ts_params(&m.params);
                if let Some(t) = &m.return_type {
                    self.collect_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSIndexSignature(i) => {
                self.collect_ts_ann(&i.parameter.type_annotation);
                self.collect_ts_ann(&i.type_annotation);
            }
            oxc::ast::ast::TSSignature::TSCallSignatureDeclaration(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.collect_ts_type_params(tp);
                }
                if let Some(this) = &c.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.collect_ts_ann(t);
                    }
                }
                self.collect_ts_params(&c.params);
                if let Some(t) = &c.return_type {
                    self.collect_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSConstructSignatureDeclaration(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.collect_ts_type_params(tp);
                }
                self.collect_ts_params(&c.params);
                if let Some(t) = &c.return_type {
                    self.collect_ts_ann(t);
                }
            }
        }
    }

    fn collect_class_named_sig(
        &mut self,
        cname: Option<&str>,
        is_static: bool,
        key: &oxc::ast::ast::PropertyKey<'_>,
        span_start: u32,
        value_start: u32,
    ) {
        let Some(cname) = cname else {
            return;
        };
        let Some(meth) = prop_key_name(key) else {
            return;
        };
        let mut offs = vec![span_start, value_start];
        if let oxc::ast::ast::PropertyKey::StaticIdentifier(id) = key {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            if is_static {
                self.insert_sig(format!("{cname}.{meth}"), sig);
            } else {
                self.insert_sig(format!("{cname}#{meth}"), sig.clone());
                self.insert_sig(format!("{cname}.{meth}"), sig);
            }
        }
    }

    fn collect_object_methods(&mut self, obj: &oxc::ast::ast::ObjectExpression<'_>, prefix: &str) {
        for p in &obj.properties {
            match p {
                oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                    let Some(name) = prop_key_name(&p.key) else {
                        continue;
                    };
                    let mut offs = vec![p.span.start, p.value.span().start];
                    if let oxc::ast::ast::PropertyKey::StaticIdentifier(id) = &p.key {
                        offs.push(id.span.start);
                    }
                    if let Some(sig) = self.type_sig_at(&offs) {
                        self.insert_sig(format!("{prefix}.{name}"), sig);
                    }
                    self.collect_object_methods_from(&p.value, &format!("{prefix}.{name}"));
                }
                oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                    self.collect_object_methods_from(&s.argument, prefix);
                }
            }
        }
    }

    fn collect_object_methods_from(&mut self, expr: &Expression<'_>, prefix: &str) {
        match peel(expr) {
            Expression::ObjectExpression(o) => self.collect_object_methods(o, prefix),
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.collect_object_methods_from(&s.argument, prefix);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_object_methods_from(e, prefix);
                            }
                        }
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_object_methods_from(e, prefix);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_object_methods_from(&b.left, prefix);
                self.collect_object_methods_from(&b.right, prefix);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_object_methods_from(&c.consequent, prefix);
                self.collect_object_methods_from(&c.alternate, prefix);
            }
            Expression::ClassExpression(c) => self.collect_class(c, Some(prefix)),
            Expression::AssignmentExpression(a) => {
                self.collect_object_methods_from(&a.right, prefix)
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_object_methods_from(arg, prefix);
                }
            }
            Expression::FunctionExpression(f) => {
                self.collect_fn_under_name(f, prefix, None);
            }
            Expression::ArrowFunctionExpression(a) => {
                self.collect_arrow_under_name(a, prefix);
            }
            _ => {}
        }
    }

    fn collect_fn_under_name(&mut self, func: &Function<'_>, name: &str, extra: Option<u32>) {
        let mut offs = vec![func.span.start];
        if let Some(e) = extra {
            offs.push(e);
        }
        if let Some(id) = &func.id {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            self.insert_sig(name.to_string(), sig);
        }
        self.collect_fn(func, extra);
    }

    fn collect_arrow_under_name(&mut self, arrow: &ArrowFunctionExpression<'_>, name: &str) {
        if let Some(sig) = self.type_sig_at(&[arrow.span.start]) {
            self.insert_sig(name.to_string(), sig);
        }
        self.collect_sigs_arrow(arrow);
    }

    fn collect_param_default_object_methods(
        &mut self,
        params: &oxc::ast::ast::FormalParameters<'_>,
    ) {
        for p in &params.items {
            if let Some(t) = &p.type_annotation {
                self.collect_ts_ann(t);
            }
            self.collect_binding_defaults(&p.pattern);
            if let Some(init) = &p.initializer {
                self.collect_sigs_expr(init);
                self.collect_binding_object_methods(&p.pattern, init);
            }
        }
        if let Some(r) = &params.rest {
            self.collect_sigs_decorators(&r.decorators);
            if let Some(t) = &r.type_annotation {
                self.collect_ts_ann(t);
            }
            self.collect_binding_defaults(&r.rest.argument);
        }
    }

    fn collect_for_left_object_methods(
        &mut self,
        left: &oxc::ast::ast::ForStatementLeft<'_>,
        right: &Expression<'_>,
    ) {
        match left {
            oxc::ast::ast::ForStatementLeft::VariableDeclaration(v) => {
                self.collect_var(v, None);
                for d in &v.declarations {
                    self.collect_iter_binding_object_methods(&d.id, right);
                }
            }
            other => {
                if let Some(t) = other.as_assignment_target() {
                    self.collect_sigs_assignment_target(t);
                    self.collect_iter_assignment_object_methods(t, right);
                }
            }
        }
    }

    fn collect_iter_binding_object_methods(
        &mut self,
        pat: &BindingPattern<'_>,
        rhs: &Expression<'_>,
    ) {
        match peel(rhs) {
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.collect_iter_binding_object_methods(pat, &s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_binding_object_methods(pat, e);
                            }
                        }
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_iter_binding_object_methods(pat, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_iter_binding_object_methods(pat, &b.left);
                self.collect_iter_binding_object_methods(pat, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_iter_binding_object_methods(pat, &c.consequent);
                self.collect_iter_binding_object_methods(pat, &c.alternate);
            }
            Expression::AssignmentExpression(a) => {
                self.collect_iter_binding_object_methods(pat, &a.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_iter_binding_object_methods(pat, arg);
                }
            }
            _ => self.collect_binding_object_methods(pat, rhs),
        }
    }

    fn collect_iter_assignment_object_methods(
        &mut self,
        t: &oxc::ast::ast::AssignmentTarget<'_>,
        rhs: &Expression<'_>,
    ) {
        match peel(rhs) {
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.collect_iter_assignment_object_methods(t, &s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.collect_assignment_object_methods(t, e);
                            }
                        }
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_iter_assignment_object_methods(t, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_iter_assignment_object_methods(t, &b.left);
                self.collect_iter_assignment_object_methods(t, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_iter_assignment_object_methods(t, &c.consequent);
                self.collect_iter_assignment_object_methods(t, &c.alternate);
            }
            Expression::AssignmentExpression(a) => {
                self.collect_iter_assignment_object_methods(t, &a.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_iter_assignment_object_methods(t, arg);
                }
            }
            _ => self.collect_assignment_object_methods(t, rhs),
        }
    }

    fn collect_object_prop_into_pat(
        &mut self,
        pat: &BindingPattern<'_>,
        p: &oxc::ast::ast::ObjectProperty<'_>,
    ) {
        let mut offs = vec![p.span.start, p.value.span().start];
        if let oxc::ast::ast::PropertyKey::StaticIdentifier(id) = &p.key {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            if let Some(name) = ident_of_pattern(pat) {
                self.insert_sig(name, sig);
            }
        }
        self.collect_binding_object_methods(pat, &p.value);
    }

    fn collect_object_prop_into_name(&mut self, name: &str, p: &oxc::ast::ast::ObjectProperty<'_>) {
        let mut offs = vec![p.span.start, p.value.span().start];
        if let oxc::ast::ast::PropertyKey::StaticIdentifier(id) = &p.key {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            self.insert_sig(name.to_string(), sig);
        }
        self.collect_object_methods_from(&p.value, name);
    }

    fn collect_binding_object_methods(&mut self, pat: &BindingPattern<'_>, init: &Expression<'_>) {
        match pat {
            BindingPattern::BindingIdentifier(id) => {
                self.collect_object_methods_from(init, id.name.as_str());
            }
            BindingPattern::AssignmentPattern(a) => {
                self.collect_binding_object_methods(&a.left, &a.right);
                self.collect_binding_object_methods(&a.left, init);
            }
            BindingPattern::ArrayPattern(a) => {
                self.collect_binding_defaults(pat);
                self.collect_array_pattern_methods_from(a, 0, init);
            }
            BindingPattern::ObjectPattern(o) => {
                self.collect_binding_defaults(pat);
                self.collect_object_pattern_methods(o, init);
            }
        }
    }

    fn collect_sigs_assignment_target(&mut self, t: &oxc::ast::ast::AssignmentTarget<'_>) {
        match t {
            oxc::ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                self.collect_sigs_expr(&m.object);
                self.collect_sigs_expr(&m.expression);
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                self.collect_sigs_expr(&m.object)
            }
            oxc::ast::ast::AssignmentTarget::PrivateFieldExpression(m) => {
                self.collect_sigs_expr(&m.object)
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.collect_sigs_atmd(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.collect_sigs_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                self.collect_sigs_expr(init);
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            if let Some(k) = p.name.as_expression() {
                                self.collect_sigs_expr(k);
                            }
                            self.collect_sigs_atmd(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.collect_sigs_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => {
                self.collect_sigs_expr(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                self.collect_sigs_expr(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                self.collect_sigs_expr(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => {
                self.collect_sigs_expr(&e.expression)
            }
            _ => {}
        }
    }

    fn collect_sigs_atmd(&mut self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.collect_sigs_expr(&d.init);
                self.collect_sigs_assignment_target(&d.binding);
            }
            other => {
                if let Some(at) = other.as_assignment_target() {
                    self.collect_sigs_assignment_target(at);
                }
            }
        }
    }

    fn collect_assignment_object_methods(
        &mut self,
        t: &oxc::ast::ast::AssignmentTarget<'_>,
        init: &Expression<'_>,
    ) {
        match t {
            oxc::ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                self.collect_object_methods_from(init, id.name.as_str());
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                if let Some(obj) = callee_name(&m.object) {
                    self.collect_object_methods_from(
                        init,
                        &format!("{}.{}", obj, m.property.name.as_str()),
                    );
                }
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                self.collect_assignment_defaults(t);
                self.collect_array_assignment_methods_from(a, 0, init);
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                self.collect_assignment_defaults(t);
                self.collect_object_assignment_methods(o, init);
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => {
                if let Some(prefix) = callee_name(&e.expression) {
                    self.collect_object_methods_from(init, &prefix);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                if let Some(prefix) = callee_name(&e.expression) {
                    self.collect_object_methods_from(init, &prefix);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                if let Some(prefix) = callee_name(&e.expression) {
                    self.collect_object_methods_from(init, &prefix);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => {
                if let Some(prefix) = callee_name(&e.expression) {
                    self.collect_object_methods_from(init, &prefix);
                }
            }
            _ => {
                if let Some(prefix) = assignment_target_prefix(t) {
                    self.collect_object_methods_from(init, &prefix);
                }
            }
        }
    }

    fn collect_atmd_object_methods(
        &mut self,
        t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>,
        init: &Expression<'_>,
    ) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.collect_assignment_object_methods(&d.binding, &d.init);
                self.collect_assignment_object_methods(&d.binding, init);
            }
            other => {
                if let Some(at) = other.as_assignment_target() {
                    self.collect_assignment_object_methods(at, init);
                }
            }
        }
    }

    fn collect_assignment_defaults(&mut self, t: &oxc::ast::ast::AssignmentTarget<'_>) {
        match t {
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.collect_atmd_defaults(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.collect_assignment_defaults(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(def) = &p.init {
                                self.collect_object_methods_from(def, p.binding.name.as_str());
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.collect_atmd_defaults(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.collect_assignment_defaults(&r.target);
                }
            }
            _ => {}
        }
    }

    fn collect_atmd_defaults(&mut self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.collect_assignment_object_methods(&d.binding, &d.init);
            }
            other => {
                if let Some(at) = other.as_assignment_target() {
                    self.collect_assignment_defaults(at);
                }
            }
        }
    }

    fn collect_binding_defaults(&mut self, pat: &BindingPattern<'_>) {
        match pat {
            BindingPattern::AssignmentPattern(a) => {
                self.collect_sigs_expr(&a.right);
                self.collect_binding_object_methods(&a.left, &a.right);
            }
            BindingPattern::ArrayPattern(a) => {
                for el in &a.elements {
                    if let Some(p) = el {
                        self.collect_binding_defaults(p);
                    }
                }
                if let Some(r) = &a.rest {
                    self.collect_binding_defaults(&r.argument);
                }
            }
            BindingPattern::ObjectPattern(o) => {
                for bp in &o.properties {
                    if let Some(k) = bp.key.as_expression() {
                        self.collect_sigs_expr(k);
                    }
                    self.collect_binding_defaults(&bp.value);
                }
                if let Some(r) = &o.rest {
                    self.collect_binding_defaults(&r.argument);
                }
            }
            BindingPattern::BindingIdentifier(_) => {}
        }
    }

    fn collect_array_pattern_methods_from(
        &mut self,
        a: &oxc::ast::ast::ArrayPattern<'_>,
        start: usize,
        init: &Expression<'_>,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_array_pattern_from_flat(a, start, &flat);
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_array_pattern_methods_from(a, start, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_array_pattern_methods_from(a, start, &b.left);
                self.collect_array_pattern_methods_from(a, start, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_array_pattern_methods_from(a, start, &c.consequent);
                self.collect_array_pattern_methods_from(a, start, &c.alternate);
            }
            Expression::AssignmentExpression(asgn) => {
                self.collect_array_pattern_methods_from(a, start, &asgn.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_array_pattern_methods_from(a, start, arg);
                }
            }
            _ => {}
        }
    }

    fn collect_array_pattern_from_flat(
        &mut self,
        a: &oxc::ast::ast::ArrayPattern<'_>,
        start: usize,
        flat: &[FlatArrayEl<'_>],
    ) {
        let mut pat_i = start;
        let mut fi = 0usize;
        while fi < flat.len() {
            match flat[fi] {
                FlatArrayEl::Spread(arg) => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    match peel(arg) {
                        Expression::LogicalExpression(b) => {
                            self.collect_array_pattern_methods_from(a, pat_i, &b.left);
                            self.collect_array_pattern_methods_from(a, pat_i, &b.right);
                            pat_i += known_flat_len(arg).unwrap_or(0);
                            fi += 1;
                        }
                        Expression::ConditionalExpression(c) => {
                            self.collect_array_pattern_methods_from(a, pat_i, &c.consequent);
                            self.collect_array_pattern_methods_from(a, pat_i, &c.alternate);
                            pat_i += known_flat_len(arg).unwrap_or(0);
                            fi += 1;
                        }
                        _ => {
                            self.collect_array_pattern_methods_from(a, pat_i, arg);
                            return;
                        }
                    }
                }
                FlatArrayEl::Hole => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    pat_i += 1;
                    fi += 1;
                }
                FlatArrayEl::Expr(e) => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    if let Some(p) = a.elements[pat_i].as_ref() {
                        self.collect_binding_object_methods(p, e);
                    }
                    pat_i += 1;
                    fi += 1;
                }
            }
        }
        if let Some(rest) = &a.rest {
            self.collect_rest_from_flat(&rest.argument, &flat[fi..]);
        }
    }

    fn collect_rest_from_flat(&mut self, pat: &BindingPattern<'_>, flat: &[FlatArrayEl<'_>]) {
        match pat {
            BindingPattern::BindingIdentifier(id) => {
                for el in flat {
                    match el {
                        FlatArrayEl::Expr(e) => {
                            self.collect_object_methods_from(e, id.name.as_str());
                        }
                        FlatArrayEl::Spread(arg) => {
                            self.collect_object_methods_from(arg, id.name.as_str());
                        }
                        FlatArrayEl::Hole => {}
                    }
                }
            }
            BindingPattern::ArrayPattern(inner) => {
                self.collect_array_pattern_from_flat(inner, 0, flat);
            }
            BindingPattern::AssignmentPattern(a) => {
                self.collect_binding_object_methods(&a.left, &a.right);
                self.collect_rest_from_flat(&a.left, flat);
            }
            BindingPattern::ObjectPattern(o) => {
                self.collect_object_pattern_from_flat_at(o, flat, 0);
            }
        }
    }

    fn collect_object_pattern_from_flat_at(
        &mut self,
        o: &oxc::ast::ast::ObjectPattern<'_>,
        flat: &[FlatArrayEl<'_>],
        origin: usize,
    ) {
        let mut i = origin;
        for el in flat {
            match el {
                FlatArrayEl::Expr(e) => {
                    self.collect_object_pattern_at_index(o, i, e);
                    i += 1;
                }
                FlatArrayEl::Hole => i += 1,
                FlatArrayEl::Spread(arg) => match peel(arg) {
                    Expression::LogicalExpression(b) => {
                        self.collect_object_pattern_methods_at(o, &b.left, i);
                        self.collect_object_pattern_methods_at(o, &b.right, i);
                        i += known_flat_len(arg).unwrap_or(0);
                    }
                    Expression::ConditionalExpression(c) => {
                        self.collect_object_pattern_methods_at(o, &c.consequent, i);
                        self.collect_object_pattern_methods_at(o, &c.alternate, i);
                        i += known_flat_len(arg).unwrap_or(0);
                    }
                    Expression::ArrayExpression(arr) => {
                        let mut inner = Vec::new();
                        flatten_array_elements(&arr.elements, &mut inner);
                        self.collect_object_pattern_from_flat_at(o, &inner, i);
                        i += inner.len();
                    }
                    _ => self.collect_object_pattern_methods_at(o, arg, i),
                },
            }
        }
    }

    fn collect_object_pattern_methods_at(
        &mut self,
        o: &oxc::ast::ast::ObjectPattern<'_>,
        init: &Expression<'_>,
        origin: usize,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_object_pattern_from_flat_at(o, &flat, origin);
            }
            Expression::LogicalExpression(b) => {
                self.collect_object_pattern_methods_at(o, &b.left, origin);
                self.collect_object_pattern_methods_at(o, &b.right, origin);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_object_pattern_methods_at(o, &c.consequent, origin);
                self.collect_object_pattern_methods_at(o, &c.alternate, origin);
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_object_pattern_methods_at(o, e, origin);
                }
            }
            Expression::AssignmentExpression(a) => {
                self.collect_object_pattern_methods_at(o, &a.right, origin);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_object_pattern_methods_at(o, arg, origin);
                }
            }
            _ => self.collect_object_pattern_methods(o, init),
        }
    }

    fn collect_object_pattern_at_index(
        &mut self,
        o: &oxc::ast::ast::ObjectPattern<'_>,
        i: usize,
        e: &Expression<'_>,
    ) {
        let key = i.to_string();
        for bp in &o.properties {
            if prop_key_name(&bp.key).as_deref() == Some(key.as_str()) {
                self.collect_binding_object_methods(&bp.value, e);
            }
        }
    }

    fn collect_object_pattern_methods(
        &mut self,
        o: &oxc::ast::ast::ObjectPattern<'_>,
        init: &Expression<'_>,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_object_pattern_from_flat_at(o, &flat, 0);
            }
            Expression::ObjectExpression(obj) => {
                for bp in &o.properties {
                    let Some(key) = prop_key_name(&bp.key) else {
                        continue;
                    };
                    for p in &obj.properties {
                        match p {
                            oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                                if prop_key_name(&p.key).as_deref() == Some(key.as_str()) {
                                    self.collect_object_prop_into_pat(&bp.value, p);
                                }
                            }
                            oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                                self.collect_object_pattern_methods(o, &s.argument);
                            }
                        }
                    }
                }
                if let Some(rest) = &o.rest {
                    if let BindingPattern::BindingIdentifier(id) = &rest.argument {
                        self.collect_object_methods(obj, id.name.as_str());
                    } else {
                        self.collect_binding_object_methods(&rest.argument, init);
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_object_pattern_methods(o, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_object_pattern_methods(o, &b.left);
                self.collect_object_pattern_methods(o, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_object_pattern_methods(o, &c.consequent);
                self.collect_object_pattern_methods(o, &c.alternate);
            }
            Expression::AssignmentExpression(a) => {
                self.collect_object_pattern_methods(o, &a.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_object_pattern_methods(o, arg);
                }
            }
            _ => {}
        }
    }

    fn collect_array_assignment_methods_from(
        &mut self,
        a: &oxc::ast::ast::ArrayAssignmentTarget<'_>,
        start: usize,
        init: &Expression<'_>,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_array_assignment_from_flat(a, start, &flat);
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_array_assignment_methods_from(a, start, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_array_assignment_methods_from(a, start, &b.left);
                self.collect_array_assignment_methods_from(a, start, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_array_assignment_methods_from(a, start, &c.consequent);
                self.collect_array_assignment_methods_from(a, start, &c.alternate);
            }
            Expression::AssignmentExpression(asgn) => {
                self.collect_array_assignment_methods_from(a, start, &asgn.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_array_assignment_methods_from(a, start, arg);
                }
            }
            _ => {}
        }
    }

    fn collect_array_assignment_from_flat(
        &mut self,
        a: &oxc::ast::ast::ArrayAssignmentTarget<'_>,
        start: usize,
        flat: &[FlatArrayEl<'_>],
    ) {
        let mut pat_i = start;
        let mut fi = 0usize;
        while fi < flat.len() {
            match flat[fi] {
                FlatArrayEl::Spread(arg) => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    match peel(arg) {
                        Expression::LogicalExpression(b) => {
                            self.collect_array_assignment_methods_from(a, pat_i, &b.left);
                            self.collect_array_assignment_methods_from(a, pat_i, &b.right);
                            pat_i += known_flat_len(arg).unwrap_or(0);
                            fi += 1;
                        }
                        Expression::ConditionalExpression(c) => {
                            self.collect_array_assignment_methods_from(a, pat_i, &c.consequent);
                            self.collect_array_assignment_methods_from(a, pat_i, &c.alternate);
                            pat_i += known_flat_len(arg).unwrap_or(0);
                            fi += 1;
                        }
                        _ => {
                            self.collect_array_assignment_methods_from(a, pat_i, arg);
                            return;
                        }
                    }
                }
                FlatArrayEl::Hole => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    pat_i += 1;
                    fi += 1;
                }
                FlatArrayEl::Expr(e) => {
                    if pat_i >= a.elements.len() {
                        break;
                    }
                    if let Some(p) = a.elements[pat_i].as_ref() {
                        self.collect_atmd_object_methods(p, e);
                    }
                    pat_i += 1;
                    fi += 1;
                }
            }
        }
        if let Some(rest) = &a.rest {
            self.collect_assignment_rest_from_flat(&rest.target, &flat[fi..]);
        }
    }

    fn collect_assignment_rest_from_flat(
        &mut self,
        t: &oxc::ast::ast::AssignmentTarget<'_>,
        flat: &[FlatArrayEl<'_>],
    ) {
        match t {
            oxc::ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                for el in flat {
                    match el {
                        FlatArrayEl::Expr(e) => {
                            self.collect_object_methods_from(e, id.name.as_str());
                        }
                        FlatArrayEl::Spread(arg) => {
                            self.collect_object_methods_from(arg, id.name.as_str());
                        }
                        FlatArrayEl::Hole => {}
                    }
                }
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(inner) => {
                self.collect_array_assignment_from_flat(inner, 0, flat);
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                self.collect_object_assignment_from_flat_at(o, flat, 0);
            }
            _ => {}
        }
    }

    fn collect_object_assignment_from_flat_at(
        &mut self,
        o: &oxc::ast::ast::ObjectAssignmentTarget<'_>,
        flat: &[FlatArrayEl<'_>],
        origin: usize,
    ) {
        let mut i = origin;
        for el in flat {
            match el {
                FlatArrayEl::Expr(e) => {
                    self.collect_object_assignment_at_index(o, i, e);
                    i += 1;
                }
                FlatArrayEl::Hole => i += 1,
                FlatArrayEl::Spread(arg) => match peel(arg) {
                    Expression::LogicalExpression(b) => {
                        self.collect_object_assignment_methods_at(o, &b.left, i);
                        self.collect_object_assignment_methods_at(o, &b.right, i);
                        i += known_flat_len(arg).unwrap_or(0);
                    }
                    Expression::ConditionalExpression(c) => {
                        self.collect_object_assignment_methods_at(o, &c.consequent, i);
                        self.collect_object_assignment_methods_at(o, &c.alternate, i);
                        i += known_flat_len(arg).unwrap_or(0);
                    }
                    Expression::ArrayExpression(arr) => {
                        let mut inner = Vec::new();
                        flatten_array_elements(&arr.elements, &mut inner);
                        self.collect_object_assignment_from_flat_at(o, &inner, i);
                        i += inner.len();
                    }
                    _ => self.collect_object_assignment_methods_at(o, arg, i),
                },
            }
        }
    }

    fn collect_object_assignment_methods_at(
        &mut self,
        o: &oxc::ast::ast::ObjectAssignmentTarget<'_>,
        init: &Expression<'_>,
        origin: usize,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_object_assignment_from_flat_at(o, &flat, origin);
            }
            Expression::LogicalExpression(b) => {
                self.collect_object_assignment_methods_at(o, &b.left, origin);
                self.collect_object_assignment_methods_at(o, &b.right, origin);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_object_assignment_methods_at(o, &c.consequent, origin);
                self.collect_object_assignment_methods_at(o, &c.alternate, origin);
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_object_assignment_methods_at(o, e, origin);
                }
            }
            Expression::AssignmentExpression(a) => {
                self.collect_object_assignment_methods_at(o, &a.right, origin);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_object_assignment_methods_at(o, arg, origin);
                }
            }
            _ => self.collect_object_assignment_methods(o, init),
        }
    }

    fn collect_object_assignment_at_index(
        &mut self,
        o: &oxc::ast::ast::ObjectAssignmentTarget<'_>,
        i: usize,
        e: &Expression<'_>,
    ) {
        let key = i.to_string();
        for p in &o.properties {
            match p {
                oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                    if p.binding.name.as_str() == key {
                        self.collect_object_methods_from(e, p.binding.name.as_str());
                    }
                }
                oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                    if prop_key_name(&p.name).as_deref() == Some(key.as_str()) {
                        self.collect_atmd_object_methods(&p.binding, e);
                    }
                }
            }
        }
    }

    fn collect_object_assignment_methods(
        &mut self,
        o: &oxc::ast::ast::ObjectAssignmentTarget<'_>,
        init: &Expression<'_>,
    ) {
        match peel(init) {
            Expression::ArrayExpression(arr) => {
                let mut flat = Vec::new();
                flatten_array_elements(&arr.elements, &mut flat);
                self.collect_object_assignment_from_flat_at(o, &flat, 0);
            }
            Expression::ObjectExpression(obj) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            let key = p.binding.name.as_str();
                            if let Some(def) = &p.init {
                                self.collect_object_methods_from(def, key);
                            }
                            for op in &obj.properties {
                                match op {
                                    oxc::ast::ast::ObjectPropertyKind::ObjectProperty(op) => {
                                        if prop_key_name(&op.key).as_deref() == Some(key) {
                                            self.collect_object_prop_into_name(key, op);
                                        }
                                    }
                                    oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                                        self.collect_object_assignment_methods(o, &s.argument);
                                    }
                                }
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            let Some(key) = prop_key_name(&p.name) else {
                                continue;
                            };
                            for op in &obj.properties {
                                match op {
                                    oxc::ast::ast::ObjectPropertyKind::ObjectProperty(op) => {
                                        if prop_key_name(&op.key).as_deref() == Some(key.as_str()) {
                                            if let Some(name) = atmd_binding_name(&p.binding) {
                                                self.collect_object_prop_into_name(&name, op);
                                            }
                                            self.collect_atmd_object_methods(&p.binding, &op.value);
                                        }
                                    }
                                    oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                                        self.collect_object_assignment_methods(o, &s.argument);
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(rest) = &o.rest {
                    self.collect_assignment_object_methods(&rest.target, init);
                }
            }
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.collect_object_assignment_methods(o, e);
                }
            }
            Expression::LogicalExpression(b) => {
                self.collect_object_assignment_methods(o, &b.left);
                self.collect_object_assignment_methods(o, &b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.collect_object_assignment_methods(o, &c.consequent);
                self.collect_object_assignment_methods(o, &c.alternate);
            }
            Expression::AssignmentExpression(a) => {
                self.collect_object_assignment_methods(o, &a.right);
            }
            Expression::YieldExpression(y) => {
                if let Some(arg) = &y.argument {
                    self.collect_object_assignment_methods(o, arg);
                }
            }
            _ => {}
        }
    }

    fn collect_object(&mut self, obj: &oxc::ast::ast::ObjectExpression<'_>) {
        for p in &obj.properties {
            match p {
                oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                    if let Some(k) = p.key.as_expression() {
                        self.collect_sigs_expr(k);
                    }
                    self.collect_sigs_expr(&p.value);
                }
                oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                    self.collect_sigs_expr(&s.argument)
                }
            }
        }
    }

    fn collect_sigs_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>) {
        self.collect_fn_scopes.push(arrow.span.start);
        if let Some(tp) = &arrow.type_parameters {
            self.collect_ts_type_params(tp);
        }
        if let Some(rt) = &arrow.return_type {
            self.collect_ts_ann(rt);
        }
        self.collect_param_default_object_methods(&arrow.params);
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => {
                for s in &body.statements {
                    self.collect_sigs_stmt(s, None);
                }
            }
            other => {
                if let Some(e) = other.as_expression() {
                    self.collect_sigs_expr(e);
                }
            }
        }
        self.collect_fn_scopes.pop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnKind {
    Unique,
    Affine,
    RefRead,
    RefWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarState {
    Unconsumed,
    BorrowedRead,
    BorrowedWrite,
    Consumed,
}

#[derive(Debug, Clone)]
struct VarEntry {
    kind: OwnKind,
    state: VarState,
    loop_depth: u32,
    defined_at: u32,
    owner: Option<String>,
    read_borrows: u32,
    write_borrows: u32,
    ty_name: String,
    opaque_aggregate: bool,
}

#[derive(Debug, Clone)]
struct ScopeBinding {
    name: String,
    shadowed: Option<VarEntry>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Apps {
    consumed: u32,
    read: u32,
    write: u32,
    path: u32,
}

impl Apps {
    fn merge(self, o: Apps) -> Apps {
        Apps {
            consumed: self.consumed + o.consumed,
            read: self.read + o.read,
            write: self.write + o.write,
            path: self.path + o.path,
        }
    }
    fn is_zero(self) -> bool {
        self.consumed == 0 && self.read == 0 && self.write == 0 && self.path == 0
    }
}

#[derive(Clone, Copy)]
enum Part {
    Zero,
    One,
    More,
}

fn part(n: u32) -> Part {
    match n {
        0 => Part::Zero,
        1 => Part::One,
        _ => Part::More,
    }
}

struct Checker<'a> {
    file: &'a FileCtx<'a>,
    diags: Vec<Diagnostic>,
    tbl: HashMap<String, VarEntry>,
    scopes: Vec<Vec<ScopeBinding>>,
    suppress_consume: Option<String>,
    loop_depth: u32,
    fn_ret: Option<OwnType>,
    try_finally_depth: u32,
    pending_finally: Vec<Option<HashMap<String, VarEntry>>>,
    /// Skip re-entry so the first annotation offsets win.
    checked_bodies: HashSet<u32>,
    callee_scopes: Vec<u32>,
    namespace_scopes: Vec<String>,
    features: OwnFeatures,
}

impl Checker<'_> {
    fn callee_sig(&self, name: &str) -> Option<&FnSig> {
        if self.features.local_callee_contracts {
            for scope in self.callee_scopes.iter().rev() {
                if let Some(sig) = self
                    .file
                    .scoped_sigs
                    .get(scope)
                    .and_then(|sigs| sigs.get(name))
                {
                    return Some(sig);
                }
            }
            for namespace in self.namespace_scopes.iter().rev() {
                if let Some(sig) = self
                    .file
                    .namespace_sigs
                    .get(namespace)
                    .and_then(|sigs| sigs.get(name))
                {
                    return Some(sig);
                }
            }
            if let Some(sig) = self
                .file
                .scoped_sigs
                .get(&FileCtx::ROOT_SIG_SCOPE)
                .and_then(|sigs| sigs.get(name))
            {
                return Some(sig);
            }
            self.file.sigs.get(name)
        } else {
            self.file.prelude_sigs.get(name)
        }
    }

    fn emit(&mut self, offset: u32, kind: RuleKind, message: impl Into<String>) {
        if kind == RuleKind::UniqueForget && !self.features.exact_once {
            return;
        }
        if kind == RuleKind::UnmappedConstruct && !self.features.unmapped_guards {
            return;
        }
        self.diags.push(Diagnostic {
            path: self.file.path.clone(),
            offset,
            kind,
            message: message.into(),
        });
    }

    fn enter_body(&mut self, span: u32) -> bool {
        self.checked_bodies.insert(span)
    }

    fn check_program(&mut self, program: &Program<'_>) {
        self.push_scope();
        for stmt in &program.body {
            self.check_stmt(stmt);
        }
        self.pop_scope();
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let bindings = self.scopes.pop().unwrap_or_default();
        for binding in bindings.into_iter().rev() {
            self.remove_var(&binding.name);
            if let Some(entry) = binding.shadowed {
                self.tbl.insert(binding.name, entry);
            }
        }
    }

    fn add_var(&mut self, name: String, entry: VarEntry) {
        let shadowed = self.tbl.insert(name.clone(), entry);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(ScopeBinding { name, shadowed });
        }
    }

    fn remove_var(&mut self, name: &str) {
        let Some(entry) = self.tbl.remove(name) else {
            return;
        };
        match entry.kind {
            OwnKind::Unique if self.features.exact_once && entry.state != VarState::Consumed => {
                self.emit(
                    entry.defined_at,
                    RuleKind::UniqueForget,
                    format!("unique value `{name}` is not consumed"),
                );
            }
            OwnKind::RefRead | OwnKind::RefWrite => {
                if let Some(owner) = entry.owner {
                    self.end_borrow(&owner, entry.kind);
                }
            }
            _ => {}
        }
    }

    fn begin_borrow(&mut self, owner: &str, alias: &str, mode: BorrowMode, span: u32) {
        if !self.features.borrow_model {
            return;
        }
        let Some(entry) = self.tbl.get(owner) else {
            self.emit(
                span,
                RuleKind::AnnotParseError,
                format!("cannot borrow unknown `{owner}`"),
            );
            return;
        };
        match (entry.state, mode) {
            (VarState::Unconsumed, BorrowMode::Read) => {}
            (VarState::BorrowedRead, BorrowMode::Read) => {}
            (VarState::Unconsumed, BorrowMode::Write) => {
                if entry.write_borrows > 0 || entry.read_borrows > 0 {
                    self.emit(
                        span,
                        RuleKind::MutBorrowConflict,
                        format!("overlapping `&mut` borrow of `{owner}`"),
                    );
                    return;
                }
            }
            (VarState::Consumed, _) => {
                self.emit(
                    span,
                    RuleKind::BorrowAfterMove,
                    format!("cannot borrow `{owner}` after it has been consumed"),
                );
                return;
            }
            (VarState::BorrowedWrite, _) | (VarState::BorrowedRead, BorrowMode::Write) => {
                self.emit(
                    span,
                    RuleKind::MutBorrowConflict,
                    format!("overlapping `&mut` borrow of `{owner}`"),
                );
                return;
            }
        }
        let kind = match mode {
            BorrowMode::Read => OwnKind::RefRead,
            BorrowMode::Write => OwnKind::RefWrite,
        };
        if let Some(e) = self.tbl.get_mut(owner) {
            match mode {
                BorrowMode::Read => {
                    e.read_borrows += 1;
                    e.state = VarState::BorrowedRead;
                }
                BorrowMode::Write => {
                    e.write_borrows += 1;
                    e.state = VarState::BorrowedWrite;
                }
            }
        }
        let ty_name = self
            .tbl
            .get(owner)
            .map(|e| e.ty_name.clone())
            .unwrap_or_default();
        self.add_var(
            alias.to_string(),
            VarEntry {
                kind,
                state: VarState::Unconsumed,
                loop_depth: self.loop_depth,
                defined_at: span,
                owner: Some(owner.to_string()),
                read_borrows: 0,
                write_borrows: 0,
                ty_name,
                opaque_aggregate: false,
            },
        );
    }

    fn end_borrow(&mut self, owner: &str, kind: OwnKind) {
        if let Some(e) = self.tbl.get_mut(owner) {
            match kind {
                OwnKind::RefRead => {
                    e.read_borrows = e.read_borrows.saturating_sub(1);
                    if e.read_borrows == 0
                        && e.write_borrows == 0
                        && matches!(e.state, VarState::BorrowedRead | VarState::BorrowedWrite)
                    {
                        e.state = VarState::Unconsumed;
                    }
                }
                OwnKind::RefWrite => {
                    e.write_borrows = e.write_borrows.saturating_sub(1);
                    if e.read_borrows == 0
                        && e.write_borrows == 0
                        && matches!(e.state, VarState::BorrowedRead | VarState::BorrowedWrite)
                    {
                        e.state = VarState::Unconsumed;
                    }
                }
                _ => {}
            }
        }
    }

    fn check_stmt(&mut self, stmt: &Statement<'_>) {
        self.apply_stmt_directives(stmt);
        match stmt {
            Statement::BlockStatement(b) => {
                self.push_scope();
                for s in &b.body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            Statement::VariableDeclaration(v) => self.check_var_decl(v, None),
            Statement::FunctionDeclaration(f) => self.check_function(f, &[f.span.start]),
            Statement::IfStatement(i) => {
                self.check_expr(&i.test, i.test.span().start);
                self.check_discard(&i.test);
                if !self.features.control_flow_splitting {
                    self.push_scope();
                    self.check_stmt(&i.consequent);
                    self.pop_scope();
                    if let Some(alt) = &i.alternate {
                        self.push_scope();
                        self.check_stmt(alt);
                        self.pop_scope();
                    }
                    return;
                }
                let saved = self.tbl.clone();
                self.push_scope();
                self.check_stmt(&i.consequent);
                self.pop_scope();
                let then_tbl = self.tbl.clone();
                self.tbl = saved.clone();
                self.push_scope();
                if let Some(alt) = &i.alternate {
                    self.check_stmt(alt);
                }
                self.pop_scope();
                let else_tbl = self.tbl.clone();
                self.tables_consistent("an if", &then_tbl, &else_tbl, i.span.start);
                self.tbl = then_tbl;
            }
            Statement::WhileStatement(w) => {
                self.loop_depth += 1;
                self.check_expr(&w.test, w.test.span().start);
                self.check_discard(&w.test);
                self.push_scope();
                self.check_stmt(&w.body);
                self.pop_scope();
                self.loop_depth -= 1;
            }
            Statement::DoWhileStatement(w) => {
                self.loop_depth += 1;
                self.push_scope();
                self.check_stmt(&w.body);
                self.pop_scope();
                self.check_expr(&w.test, w.test.span().start);
                self.check_discard(&w.test);
                self.loop_depth -= 1;
            }
            Statement::ForStatement(f) => {
                self.push_scope();
                if let Some(init) = &f.init {
                    match init {
                        ForStatementInit::VariableDeclaration(v) => self.check_var_decl(v, None),
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_expr(e, e.span().start);
                                self.check_discard(e);
                            }
                        }
                    }
                }
                self.loop_depth += 1;
                if let Some(test) = &f.test {
                    self.check_expr(test, test.span().start);
                    self.check_discard(test);
                }
                if let Some(upd) = &f.update {
                    self.check_expr(upd, upd.span().start);
                    self.check_discard(upd);
                }
                self.check_stmt(&f.body);
                self.loop_depth -= 1;
                self.pop_scope();
            }
            Statement::ForInStatement(f) => {
                self.check_for_left(&f.left);
                self.check_expr(&f.right, f.right.span().start);
                self.check_discard(&f.right);
                self.loop_depth += 1;
                self.push_scope();
                self.check_stmt(&f.body);
                self.pop_scope();
                self.loop_depth -= 1;
            }
            Statement::ForOfStatement(f) => {
                self.check_for_left(&f.left);
                self.check_expr(&f.right, f.right.span().start);
                self.check_discard(&f.right);
                self.loop_depth += 1;
                self.push_scope();
                self.check_stmt(&f.body);
                self.pop_scope();
                self.loop_depth -= 1;
            }
            Statement::ReturnStatement(r) => {
                if let Some(arg) = &r.argument {
                    if let Expression::Identifier(id) = arg {
                        let n = id.name.as_str();
                        if let Some(e) = self.tbl.get(n) {
                            if matches!(e.kind, OwnKind::RefRead | OwnKind::RefWrite) {
                                self.emit(
                                    id.span.start,
                                    RuleKind::BorrowEscape,
                                    format!("borrow `{n}` escapes its lexical region via return"),
                                );
                            }
                        }
                    }
                    self.check_expr(arg, arg.span().start);
                    self.check_returned_unique(arg);
                }
                self.require_on_exit(r.span.start);
            }
            Statement::ThrowStatement(t) => {
                self.check_expr(&t.argument, t.argument.span().start);
                self.check_discard(&t.argument);
                self.require_on_exit(t.span.start);
            }
            Statement::ExpressionStatement(e) => {
                self.check_unmapped_eval(&e.expression);
                let transfer = self.discarded_assignment_transfer(&e.expression);
                let self_assignment_source = transfer.as_ref().and_then(
                    |(source, destination, _)| (source == destination).then(|| source.clone()),
                );
                let saved_suppression = self_assignment_source
                    .as_ref()
                    .and_then(|source| self.suppress_consume.replace(source.clone()));
                self.check_expr(&e.expression, e.span.start);
                if self_assignment_source.is_some() {
                    self.suppress_consume = saved_suppression;
                } else if let Some((source, destination, entry)) = transfer {
                    self.finish_discarded_assignment_transfer(
                        &source,
                        &destination,
                        entry,
                        e.span.start,
                    );
                }
                self.check_discard(&e.expression);
            }
            Statement::SwitchStatement(s) => {
                self.check_expr(&s.discriminant, s.discriminant.span().start);
                self.check_discard(&s.discriminant);
                if !self.features.control_flow_splitting {
                    for case in &s.cases {
                        if let Some(test) = &case.test {
                            self.check_expr(test, test.span().start);
                            self.check_discard(test);
                        }
                        self.push_scope();
                        for st in &case.consequent {
                            self.check_stmt(st);
                        }
                        self.pop_scope();
                    }
                    return;
                }
                let base = self.tbl.clone();
                let mut tables: Vec<HashMap<String, VarEntry>> = Vec::new();
                let has_default = s.cases.iter().any(|c| c.test.is_none());
                for (i, case) in s.cases.iter().enumerate() {
                    self.tbl = base.clone();
                    if let Some(test) = &case.test {
                        self.check_expr(test, test.span().start);
                        self.check_discard(test);
                    }
                    if case.consequent.is_empty() {
                        continue;
                    }
                    if i + 1 < s.cases.len() && case_falls_through(case) {
                        self.emit(
                            case.span.start,
                            RuleKind::UnmappedConstruct,
                            "switch fall-through is not mapped from Austral linearity",
                        );
                    }
                    self.push_scope();
                    for st in &case.consequent {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
                    tables.push(self.tbl.clone());
                }
                if !has_default {
                    tables.push(base.clone());
                }
                for w in tables.windows(2) {
                    self.tables_consistent("a switch", &w[0], &w[1], s.span.start);
                }
                if let Some(t) = tables.first() {
                    self.tbl = t.clone();
                }
            }
            Statement::TryStatement(t) => {
                if !self.features.control_flow_splitting {
                    self.push_scope();
                    for s in &t.block.body {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
                    if let Some(h) = &t.handler {
                        self.push_scope();
                        if let Some(p) = &h.param {
                            self.check_discard_binding(&p.pattern);
                        }
                        for s in &h.body.body {
                            self.check_stmt(s);
                        }
                        self.pop_scope();
                    }
                    if let Some(fin) = &t.finalizer {
                        self.push_scope();
                        for s in &fin.body {
                            self.check_stmt(s);
                        }
                        self.pop_scope();
                    }
                    return;
                }
                let has_finally = t.finalizer.is_some();
                if has_finally {
                    self.try_finally_depth += 1;
                    self.pending_finally.push(None);
                }
                let base = self.tbl.clone();
                self.push_scope();
                for s in &t.block.body {
                    self.check_stmt(s);
                }
                self.pop_scope();
                let try_tbl = self.tbl.clone();
                if let Some(h) = &t.handler {
                    self.tbl = base.clone();
                    self.push_scope();
                    if let Some(p) = &h.param {
                        self.check_discard_binding(&p.pattern);
                    }
                    for s in &h.body.body {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
                    let catch_tbl = self.tbl.clone();
                    self.tables_consistent("a try/catch", &try_tbl, &catch_tbl, t.span.start);
                    self.tbl = try_tbl.clone();
                }
                if let Some(fin) = &t.finalizer {
                    self.try_finally_depth = self.try_finally_depth.saturating_sub(1);
                    let exiting = self.pending_finally.pop().flatten();
                    let fallthrough = self.tbl.clone();
                    if let Some(saved) = exiting.as_ref() {
                        self.tbl = saved.clone();
                    }
                    self.push_scope();
                    for s in &fin.body {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
                    if exiting.is_some() {
                        self.propagate_tbl_to_pending();
                    }
                    if exiting.is_some() && self.try_finally_depth == 0 {
                        self.require_consumed_uniques(t.span.start, true);
                    }
                    if let Some(saved) = exiting.as_ref() {
                        if saved.iter().any(|(n, e)| {
                            fallthrough
                                .get(n)
                                .map(|f| f.state != e.state)
                                .unwrap_or(false)
                        }) {
                            self.tbl = fallthrough;
                            self.push_scope();
                            for s in &fin.body {
                                self.check_stmt(s);
                            }
                            self.pop_scope();
                        }
                    }
                }
            }
            Statement::ClassDeclaration(c) => self.check_class(c),
            Statement::WithStatement(w) => {
                self.emit(
                    w.span.start,
                    RuleKind::UnmappedConstruct,
                    "`with` statements are not mapped from Austral linearity",
                );
                self.check_expr(&w.object, w.object.span().start);
                self.check_discard(&w.object);
                self.check_stmt(&w.body);
            }
            Statement::LabeledStatement(l) => self.check_stmt(&l.body),
            Statement::ExportDeclaration(e) => self.check_decl(&e.declaration, e.span.start),
            Statement::ExportDefaultDeclaration(e) => match &e.declaration {
                oxc::ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f)
                | oxc::ast::ast::ExportDefaultDeclarationKind::FunctionExpression(f) => {
                    self.check_function(f, &[f.span.start, e.span.start]);
                }
                oxc::ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(c)
                | oxc::ast::ast::ExportDefaultDeclarationKind::ClassExpression(c) => {
                    self.check_class(c);
                }
                other => {
                    if let Some(expr) = other.as_expression() {
                        self.check_fn_or_arrow_init(expr, &[e.span.start, expr.span().start]);
                        self.check_expr(expr, expr.span().start);
                        self.check_discard(expr);
                    }
                }
            },
            Statement::TSEnumDeclaration(e) => self.check_ts_enum(e),
            Statement::TSNamespaceDeclaration(n) => self.check_ts_namespace(n),
            Statement::TSGlobalDeclaration(g) => self.check_ts_module_block(&g.body),
            Statement::TSExternalModuleDeclaration(m) => {
                if let Some(b) = &m.body {
                    self.check_ts_module_block(b);
                }
            }
            Statement::TSExportAssignment(e) => {
                self.check_expr(&e.expression, e.expression.span().start);
                self.check_discard(&e.expression);
            }
            Statement::TSInterfaceDeclaration(i) => self.check_ts_interface(i),
            Statement::TSTypeAliasDeclaration(t) => {
                if let Some(tp) = &t.type_parameters {
                    self.check_ts_type_params(tp);
                }
                self.check_ts_type(&t.type_annotation);
            }
            _ => {}
        }
    }

    fn check_for_left(&mut self, left: &oxc::ast::ast::ForStatementLeft<'_>) {
        match left {
            oxc::ast::ast::ForStatementLeft::VariableDeclaration(v) => self.check_var_decl(v, None),
            other => {
                if let Some(t) = other.as_assignment_target() {
                    self.check_expr_assignment_target(t);
                    self.check_discard_assignment_target(t);
                }
            }
        }
    }

    fn check_expr_assignment_target(&mut self, t: &oxc::ast::ast::AssignmentTarget<'_>) {
        match t {
            oxc::ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                self.check_expr(&m.object, m.object.span().start);
                self.check_expr(&m.expression, m.expression.span().start);
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                self.check_expr(&m.object, m.object.span().start);
            }
            oxc::ast::ast::AssignmentTarget::PrivateFieldExpression(m) => {
                self.check_expr(&m.object, m.object.span().start);
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.check_expr_atmd(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.check_expr_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                self.check_expr(init, init.span().start);
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            if let Some(k) = p.name.as_expression() {
                                self.check_expr(k, k.span().start);
                            }
                            self.check_expr_atmd(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.check_expr_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => {
                self.check_expr(&e.expression, e.expression.span().start)
            }
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                self.check_expr(&e.expression, e.expression.span().start)
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                self.check_expr(&e.expression, e.expression.span().start)
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => {
                self.check_expr(&e.expression, e.expression.span().start)
            }
            _ => {}
        }
    }

    fn check_expr_atmd(&mut self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.check_expr(&d.init, d.init.span().start);
                self.check_expr_assignment_target(&d.binding);
            }
            _ => {
                if let Some(x) = t.as_assignment_target() {
                    self.check_expr_assignment_target(x);
                }
            }
        }
    }

    fn visit_exclusive_assignment_target(&mut self, t: &oxc::ast::ast::AssignmentTarget<'_>) {
        match t {
            oxc::ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                self.visit_exclusive_maybe(&m.object);
                self.visit_exclusive_maybe(&m.expression);
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                self.visit_exclusive_maybe(&m.object)
            }
            oxc::ast::ast::AssignmentTarget::PrivateFieldExpression(m) => {
                self.visit_exclusive_maybe(&m.object)
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.visit_exclusive_atmd(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.visit_exclusive_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                self.visit_exclusive_maybe(init);
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            if let Some(k) = p.name.as_expression() {
                                self.visit_exclusive_maybe(k);
                            }
                            self.visit_exclusive_atmd(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.visit_exclusive_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => {
                self.visit_exclusive_maybe(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                self.visit_exclusive_maybe(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                self.visit_exclusive_maybe(&e.expression)
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => {
                self.visit_exclusive_maybe(&e.expression)
            }
            _ => {}
        }
    }

    fn visit_exclusive_atmd(&mut self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.visit_exclusive_maybe(&d.init);
                self.visit_exclusive_assignment_target(&d.binding);
            }
            other => {
                if let Some(x) = other.as_assignment_target() {
                    self.visit_exclusive_assignment_target(x);
                }
            }
        }
    }

    fn check_decl(&mut self, decl: &oxc::ast::ast::Declaration<'_>, extra: u32) {
        match decl {
            oxc::ast::ast::Declaration::FunctionDeclaration(f) => {
                self.check_function(f, &[f.span.start, extra]);
            }
            oxc::ast::ast::Declaration::ClassDeclaration(c) => self.check_class(c),
            oxc::ast::ast::Declaration::VariableDeclaration(v) => {
                self.check_var_decl(v, Some(extra))
            }
            oxc::ast::ast::Declaration::TSEnumDeclaration(e) => self.check_ts_enum(e),
            oxc::ast::ast::Declaration::TSNamespaceDeclaration(n) => self.check_ts_namespace(n),
            oxc::ast::ast::Declaration::TSGlobalDeclaration(g) => {
                self.check_ts_module_block(&g.body)
            }
            oxc::ast::ast::Declaration::TSExternalModuleDeclaration(m) => {
                if let Some(b) = &m.body {
                    self.check_ts_module_block(b);
                }
            }
            oxc::ast::ast::Declaration::TSInterfaceDeclaration(i) => self.check_ts_interface(i),
            oxc::ast::ast::Declaration::TSTypeAliasDeclaration(t) => {
                if let Some(tp) = &t.type_parameters {
                    self.check_ts_type_params(tp);
                }
                self.check_ts_type(&t.type_annotation)
            }
            _ => {}
        }
    }

    fn apply_stmt_directives(&mut self, stmt: &Statement<'_>) {
        // Variable declarations apply kind/borrow/clone directives themselves.
        if matches!(stmt, Statement::VariableDeclaration(_)) {
            return;
        }
        let start = stmt.span().start;
        let dirs: Vec<OwnDirective> = self.file.dirs_at(start).into_iter().cloned().collect();
        for d in dirs {
            match d {
                OwnDirective::Drop { name } if self.features.local_drop_directives => {
                    self.force_consume(&name, start);
                }
                OwnDirective::Borrow {
                    owner, alias, mode, ..
                } if self.features.local_borrow_directives => {
                    self.begin_borrow(&owner, &alias, mode, start);
                }
                OwnDirective::Clone { owner, alias }
                    if self.features.local_clone_directives =>
                {
                    self.do_clone(&owner, &alias, start);
                }
                OwnDirective::Let { name, ty } if self.features.local_kind_directives => {
                    // handled in var decl; if this is a bare statement, add anyway
                    if !matches!(stmt, Statement::VariableDeclaration(_)) {
                        self.add_from_type(&name, &ty, start);
                    }
                }
                _ => {}
            }
        }
    }

    fn do_clone(&mut self, owner: &str, alias: &str, span: u32) {
        let Some(entry) = self.tbl.get(owner).cloned() else {
            // cloning a copy-typed or unknown value: treat alias as copy (untracked)
            return;
        };
        if entry.state == VarState::Consumed {
            self.emit(
                span,
                RuleKind::UseAfterMove,
                format!("cannot clone `{owner}` after it has been consumed"),
            );
            return;
        }
        if matches!(
            entry.state,
            VarState::BorrowedRead | VarState::BorrowedWrite
        ) {
            self.emit(
                span,
                RuleKind::ConsumeWhileBorrowed,
                format!("cannot clone `{owner}` while it is borrowed"),
            );
            return;
        }
        self.add_var(
            alias.to_string(),
            VarEntry {
                kind: entry.kind,
                state: VarState::Unconsumed,
                loop_depth: self.loop_depth,
                defined_at: span,
                owner: None,
                read_borrows: 0,
                write_borrows: 0,
                ty_name: entry.ty_name.clone(),
                opaque_aggregate: entry.opaque_aggregate,
            },
        );
    }

    fn resolve_payload(&mut self, ty: &OwnType, span: u32, label: &str) -> OwnType {
        if !ty.payload_omitted() {
            return ty.clone();
        }
        if let Some(payloads) = self.file.payloads {
            if let Some(name) = payloads.name_at(span) {
                if let Some(name) = own_payload_name(&name).or(Some(name)) {
                    return ty.with_payload(name);
                }
            }
        }
        self.emit(
            span,
            RuleKind::MissingType,
            format!("/*#own omitted a type for `{label}` and TypeScript did not supply one"),
        );
        ty.clone()
    }

    fn add_from_type(&mut self, name: &str, ty: &OwnType, span: u32) {
        let ty = self.resolve_payload(ty, span, name);
        let (kind, ty_name) = match &ty {
            OwnType::Unique(_) | OwnType::Affine(_) if !self.features.move_tracking => return,
            OwnType::Unique(s) => (OwnKind::Unique, s.clone()),
            OwnType::Affine(s) if self.features.affine_kind => (OwnKind::Affine, s.clone()),
            OwnType::Affine(_) => return,
            OwnType::Copy(_) => return,
            OwnType::RefRead(s) if self.features.borrow_model => (OwnKind::RefRead, s.clone()),
            OwnType::RefWrite(s) if self.features.borrow_model => (OwnKind::RefWrite, s.clone()),
            OwnType::RefRead(_) | OwnType::RefWrite(_) => return,
            OwnType::Void => return,
        };
        self.add_var(
            name.to_string(),
            VarEntry {
                kind,
                state: VarState::Unconsumed,
                loop_depth: self.loop_depth,
                defined_at: span,
                owner: None,
                read_borrows: 0,
                write_borrows: 0,
                ty_name,
                opaque_aggregate: false,
            },
        );
    }

    fn force_consume(&mut self, name: &str, span: u32) {
        let mut apps = Apps::default();
        apps.consumed = 1;
        self.apply_apps(name, apps, span);
    }

    /// Infer an opaque owner for an object/array literal that directly stores
    /// owned values. The container can move as a whole; member paths remain
    /// non-consuming, so extracting a field cannot silently discharge the
    /// container without path-sensitive ownership support.
    fn aggregate_transfer(&self, expr: &Expression<'_>) -> Option<(Vec<String>, OwnKind)> {
        let mut sources = Vec::new();
        if !self.collect_aggregate_sources(expr, &mut sources) || sources.is_empty() {
            return None;
        }

        let mut seen = HashSet::new();
        if sources.iter().any(|source| !seen.insert(source.clone())) {
            // Let ordinary expression checking report the repeated move; no
            // owner is created for a transfer that did not succeed.
            return None;
        }

        let mut kind = OwnKind::Affine;
        for source in &sources {
            let entry = self.tbl.get(source)?;
            if entry.state != VarState::Unconsumed
                || !matches!(entry.kind, OwnKind::Unique | OwnKind::Affine)
            {
                return None;
            }
            if entry.kind == OwnKind::Unique {
                kind = OwnKind::Unique;
            }
        }
        Some((sources, kind))
    }

    fn collect_aggregate_sources(&self, expr: &Expression<'_>, out: &mut Vec<String>) -> bool {
        match peel(expr) {
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    match element {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(_) => {}
                        other => {
                            if let Some(value) = other.as_expression() {
                                self.collect_aggregate_value_sources(value, out);
                            }
                        }
                    }
                }
                true
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    if let oxc::ast::ast::ObjectPropertyKind::ObjectProperty(property) = property {
                        self.collect_aggregate_value_sources(&property.value, out);
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn collect_aggregate_value_sources(&self, expr: &Expression<'_>, out: &mut Vec<String>) {
        match peel(expr) {
            Expression::Identifier(identifier) => {
                let name = identifier.name.as_str();
                if self
                    .tbl
                    .get(name)
                    .is_some_and(|entry| matches!(entry.kind, OwnKind::Unique | OwnKind::Affine))
                {
                    out.push(name.to_string());
                }
            }
            Expression::ArrayExpression(_) | Expression::ObjectExpression(_) => {
                self.collect_aggregate_sources(expr, out);
            }
            _ => {}
        }
    }

    fn check_var_decl(&mut self, decl: &VariableDeclaration<'_>, extra: Option<u32>) {
        let decl_dirs: Vec<OwnDirective> = self
            .file
            .dirs_at(decl.span.start)
            .into_iter()
            .cloned()
            .collect();
        for d in &decl.declarations {
            if let Some(t) = &d.type_annotation {
                self.check_ts_ann(t);
            }
            let mut dirs = decl_dirs.clone();
            dirs.extend(self.file.dirs_at(d.span.start).into_iter().cloned());
            let name = ident_of_pattern(&d.id);
            let borrow = (self.features.borrow_model && self.features.local_borrow_directives)
            .then(|| {
                dirs.iter().find_map(|x| match x {
                    OwnDirective::Borrow {
                        owner, alias, mode, ..
                    } => Some((owner.clone(), alias.clone(), *mode)),
                    _ => None,
                })
            })
            .flatten();
            let clone = self
                .features
                .local_clone_directives
                .then(|| {
                    dirs.iter().find_map(|x| match x {
                        OwnDirective::Clone { owner, alias } => {
                            Some((owner.clone(), alias.clone()))
                        }
                        _ => None,
                    })
                })
                .flatten();
            let let_ty = self
                .features
                .local_kind_directives
                .then(|| {
                    dirs.iter().find_map(|x| match x {
                        OwnDirective::Let { name, ty } => Some((name.clone(), ty.clone())),
                        OwnDirective::Kind(ty) => {
                            Some((name.clone().unwrap_or_default(), ty.clone()))
                        }
                        _ => None,
                    })
                })
                .flatten();

            if let Some((owner, alias, mode)) = borrow {
                self.begin_borrow(&owner, &alias, mode, d.span.start);
                if let Some(init) = &d.init {
                    self.suppress_consume = Some(owner);
                    self.check_expr(init, init.span().start);
                    self.suppress_consume = None;
                }
                continue;
            }
            if let Some((owner, alias)) = clone {
                self.do_clone(&owner, &alias, d.span.start);
                if let Some(init) = &d.init {
                    self.suppress_consume = Some(owner);
                    self.check_expr(init, init.span().start);
                    self.suppress_consume = None;
                }
                continue;
            }

            if let Some(init) = &d.init {
                let mut init_offs = vec![decl.span.start, d.span.start];
                if let Some(e) = extra {
                    init_offs.push(e);
                }
                self.check_fn_or_arrow_init(init, &init_offs);
                let src_name = ident_move_src(init);
                let src_kind = src_name
                    .as_ref()
                    .and_then(|n| self.tbl.get(n).map(|e| e.kind));
                let aggregate_transfer = self.aggregate_transfer(init);
                self.check_expr(init, init.span().start);
                self.discard_sequence_prefix(init);
                if let Some(n) = &name {
                    if let Some((let_name, ty)) = &let_ty {
                        if let_name == n || let_name.is_empty() {
                            self.add_from_type(n, ty, d.span.start);
                            continue;
                        }
                    }
                    if let Some(kind) = src_kind {
                        if matches!(kind, OwnKind::Unique | OwnKind::Affine) {
                            let ty_name = src_name
                                .as_ref()
                                .and_then(|s| self.tbl.get(s).map(|e| e.ty_name.clone()))
                                .unwrap_or_default();
                            let opaque_aggregate = src_name
                                .as_ref()
                                .and_then(|s| self.tbl.get(s).map(|e| e.opaque_aggregate))
                                .unwrap_or(false);
                            self.add_var(
                                n.clone(),
                                VarEntry {
                                    kind,
                                    state: VarState::Unconsumed,
                                    loop_depth: self.loop_depth,
                                    defined_at: d.span.start,
                                    owner: None,
                                    read_borrows: 0,
                                    write_borrows: 0,
                                    ty_name,
                                    opaque_aggregate,
                                },
                            );
                            continue;
                        }
                        if matches!(kind, OwnKind::RefRead | OwnKind::RefWrite) {
                            self.emit(
                                d.span.start,
                                RuleKind::BorrowEscape,
                                format!(
                                    "borrow `{src}` escapes its lexical region by assignment to `{n}`",
                                    src = src_name.as_deref().unwrap_or("?")
                                ),
                            );
                            continue;
                        }
                    }
                    if let Some(ret) = self.call_return_type(init) {
                        self.add_from_type(n, &ret, d.span.start);
                        if matches!(ret, OwnType::Unique(_) | OwnType::Affine(_)) {
                            self.check_call_subexprs(init);
                            continue;
                        }
                    }
                    if let Some((sources, kind)) = aggregate_transfer {
                        let transferred = sources.iter().all(|source| {
                            matches!(self.tbl.get(source), Some(entry) if entry.state == VarState::Consumed)
                        });
                        if transferred {
                            self.add_var(
                                n.clone(),
                                VarEntry {
                                    kind,
                                    state: VarState::Unconsumed,
                                    loop_depth: self.loop_depth,
                                    defined_at: d.span.start,
                                    owner: None,
                                    read_borrows: 0,
                                    write_borrows: 0,
                                    ty_name: String::new(),
                                    opaque_aggregate: true,
                                },
                            );
                            // Calls that produce additional owned values are
                            // not table entries yet. Keep rejecting those as
                            // discarded rather than silently hiding them in
                            // this coarse aggregate abstraction.
                            self.check_discard(init);
                            continue;
                        }
                    }
                    if let Some((_, ty)) = &let_ty {
                        self.add_from_type(n, ty, d.span.start);
                    }
                }
            } else if let Some(n) = &name {
                if let Some((let_name, ty)) = &let_ty {
                    if let_name == n || let_name.is_empty() {
                        self.add_from_type(n, ty, d.span.start);
                    }
                }
            }

            self.check_discard_binding(&d.id);
            if let Some(init) = &d.init {
                self.discard_sequence_prefix(init);
                let bound_unique = name.is_some()
                    && matches!(self.call_return_type(init), Some(OwnType::Unique(_)));
                if bound_unique {
                    self.check_call_subexprs(init);
                } else {
                    self.check_discard(init);
                }
            }
        }
    }

    fn check_fn_or_arrow_init(&mut self, init: &Expression<'_>, extra: &[u32]) {
        match peel(init) {
            Expression::FunctionExpression(f) => self.check_function(f, extra),
            Expression::ArrowFunctionExpression(a) => {
                let mut offs = extra.to_vec();
                offs.push(a.span.start);
                if let Some(sig) = self.file.type_sig_at(&offs) {
                    self.check_arrow(a, &sig);
                } else {
                    self.check_arrow_unannotated(a);
                }
            }
            Expression::AssignmentExpression(a) => self.check_fn_or_arrow_init(&a.right, extra),
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.check_fn_or_arrow_init(e, extra);
                }
            }
            Expression::LogicalExpression(b) => {
                self.check_fn_or_arrow_init(&b.left, extra);
                self.check_fn_or_arrow_init(&b.right, extra);
            }
            Expression::ConditionalExpression(c) => {
                self.check_fn_or_arrow_init(&c.consequent, extra);
                self.check_fn_or_arrow_init(&c.alternate, extra);
            }
            _ => {}
        }
    }

    fn check_function(&mut self, func: &Function<'_>, extra: &[u32]) {
        if !self.enter_body(func.span.start) {
            return;
        }
        self.callee_scopes.push(func.span.start);
        if let Some(tp) = &func.type_parameters {
            self.check_ts_type_params(tp);
        }
        if let Some(this) = &func.this_param {
            if let Some(t) = &this.type_annotation {
                self.check_ts_ann(t);
            }
        }
        if let Some(rt) = &func.return_type {
            self.check_ts_ann(rt);
        }
        let mut offs: Vec<u32> = extra.to_vec();
        offs.push(func.span.start);
        if let Some(id) = &func.id {
            offs.push(id.span.start);
        }
        self.check_param_default_captures(func);
        if let Some(sig) = self.file.type_sig_at(&offs) {
            self.check_function_with_sig(func, &sig);
            self.callee_scopes.pop();
            return;
        }
        if let Some(body) = &func.body {
            self.scan_unmapped_body(body);
            let owned = capture_candidates_for_function(
                self.tbl.keys().cloned().collect(),
                func,
            );
            self.report_captures_in_body(body, &owned);
            let saved_tbl = self.tbl.clone();
            let saved_scopes = self.scopes.clone();
            let saved_depth = self.loop_depth;
            let saved_ret = self.fn_ret.clone();
            let saved_finally = self.try_finally_depth;
            let saved_pending = std::mem::take(&mut self.pending_finally);
            self.tbl.clear();
            self.scopes.clear();
            self.loop_depth = 0;
            self.fn_ret = None;
            self.try_finally_depth = 0;
            self.pending_finally.clear();
            self.push_scope();
            self.check_formal_params(&func.params, None);
            for s in &body.statements {
                self.check_stmt(s);
            }
            self.pop_scope();
            self.tbl = saved_tbl;
            self.scopes = saved_scopes;
            self.loop_depth = saved_depth;
            self.fn_ret = saved_ret;
            self.try_finally_depth = saved_finally;
            self.pending_finally = saved_pending;
        }
        self.callee_scopes.pop();
    }

    fn check_function_with_sig(&mut self, func: &Function<'_>, sig: &FnSig) {
        if let Some(body) = &func.body {
            let owned = capture_candidates_for_function(
                self.tbl.keys().cloned().collect(),
                func,
            );
            self.report_captures_in_body(body, &owned);
        }
        let saved_tbl = self.tbl.clone();
        let saved_scopes = self.scopes.clone();
        let saved_depth = self.loop_depth;
        let saved_ret = self.fn_ret.clone();
        let saved_finally = self.try_finally_depth;
        let saved_pending = std::mem::take(&mut self.pending_finally);
        self.tbl.clear();
        self.scopes.clear();
        self.loop_depth = 0;
        self.fn_ret = Some(self.resolve_payload(&sig.ret, func.span.start, "return"));
        self.try_finally_depth = 0;
        self.pending_finally.clear();
        self.push_scope();
        if func.body.is_some() {
            for (i, param) in func.params.items.iter().enumerate() {
                if let (Some(pname), Some((_, ty))) =
                    (ident_of_pattern(&param.pattern), sig.params.get(i))
                {
                    self.add_from_type(&pname, ty, param.span.start);
                }
            }
        }
        self.check_formal_params(&func.params, Some(sig));
        if let Some(body) = &func.body {
            for s in &body.statements {
                self.check_stmt(s);
            }
        }
        self.pop_scope();
        self.tbl = saved_tbl;
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
        self.fn_ret = saved_ret;
        self.try_finally_depth = saved_finally;
        self.pending_finally = saved_pending;
    }

    fn check_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>, sig: &FnSig) {
        if !self.enter_body(arrow.span.start) {
            return;
        }
        self.callee_scopes.push(arrow.span.start);
        if let Some(tp) = &arrow.type_parameters {
            self.check_ts_type_params(tp);
        }
        if let Some(rt) = &arrow.return_type {
            self.check_ts_ann(rt);
        }
        self.report_captures_arrow(arrow);
        let saved_tbl = self.tbl.clone();
        let saved_scopes = self.scopes.clone();
        let saved_depth = self.loop_depth;
        let saved_ret = self.fn_ret.clone();
        let saved_finally = self.try_finally_depth;
        let saved_pending = std::mem::take(&mut self.pending_finally);
        self.tbl.clear();
        self.scopes.clear();
        self.loop_depth = 0;
        self.fn_ret = Some(self.resolve_payload(&sig.ret, arrow.span.start, "return"));
        self.try_finally_depth = 0;
        self.pending_finally.clear();
        self.push_scope();
        for (i, param) in arrow.params.items.iter().enumerate() {
            if let (Some(pname), Some((_, ty))) =
                (ident_of_pattern(&param.pattern), sig.params.get(i))
            {
                self.add_from_type(&pname, ty, param.span.start);
            }
        }
        self.check_formal_params(&arrow.params, Some(sig));
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => {
                for s in &body.statements {
                    self.check_stmt(s);
                }
            }
            other => {
                if let Some(expr) = other.as_expression() {
                    if let Expression::Identifier(id) = expr {
                        let n = id.name.as_str();
                        if let Some(e) = self.tbl.get(n) {
                            if matches!(e.kind, OwnKind::RefRead | OwnKind::RefWrite) {
                                self.emit(
                                    id.span.start,
                                    RuleKind::BorrowEscape,
                                    format!("borrow `{n}` escapes its lexical region via return"),
                                );
                            }
                        }
                    }
                    self.check_expr(expr, expr.span().start);
                    self.check_returned_unique(expr);
                }
            }
        }
        self.pop_scope();
        self.tbl = saved_tbl;
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
        self.fn_ret = saved_ret;
        self.try_finally_depth = saved_finally;
        self.pending_finally = saved_pending;
        self.callee_scopes.pop();
    }

    fn propagate_tbl_to_pending(&mut self) {
        let snap = self.tbl.clone();
        for slot in self.pending_finally.iter_mut() {
            if slot.is_some() {
                *slot = Some(snap.clone());
            }
        }
    }

    fn require_on_exit(&mut self, span: u32) {
        if self.try_finally_depth > 0 {
            let snap = self.tbl.clone();
            for slot in self.pending_finally.iter_mut() {
                if slot.is_none() {
                    *slot = Some(snap.clone());
                }
            }
        } else {
            self.require_consumed_uniques(span, true);
        }
    }

    fn require_consumed_uniques(&mut self, span: u32, emit: bool) {
        if !self.features.exact_once {
            return;
        }
        let names: Vec<_> = self.tbl.keys().cloned().collect();
        for name in names {
            if let Some(e) = self.tbl.get(&name) {
                if e.kind == OwnKind::Unique && e.state != VarState::Consumed {
                    if emit {
                        self.emit(
                            span,
                            RuleKind::UniqueForget,
                            format!("unique value `{name}` is not consumed"),
                        );
                    }
                    if let Some(e) = self.tbl.get_mut(&name) {
                        e.state = VarState::Consumed;
                    }
                }
            }
        }
    }

    fn tables_consistent(
        &mut self,
        what: &str,
        a: &HashMap<String, VarEntry>,
        b: &HashMap<String, VarEntry>,
        span: u32,
    ) {
        if !self.features.control_flow_splitting {
            return;
        }
        for (name, ea) in a {
            if let Some(eb) = b.get(name) {
                if ea.state != eb.state {
                    self.emit(
                        span,
                        RuleKind::BranchInconsistent,
                        format!(
                            "variable `{name}` is used inconsistently in the branches of {what} statement"
                        ),
                    );
                }
            }
        }
    }

    fn check_discard(&mut self, expr: &Expression<'_>) {
        self.walk_type_wrappers(expr);
        match peel(expr) {
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.check_discard(e);
                }
                return;
            }
            Expression::UnaryExpression(u) if matches!(u.operator, UnaryOperator::Void) => {
                self.check_discard(&u.argument);
                return;
            }
            Expression::LogicalExpression(b) => {
                self.check_discard(&b.left);
                self.check_discard(&b.right);
                return;
            }
            Expression::ConditionalExpression(c) => {
                self.check_discard(&c.test);
                self.check_discard(&c.consequent);
                self.check_discard(&c.alternate);
                return;
            }
            Expression::AssignmentExpression(a) => {
                self.check_discard(&a.right);
                self.check_discard_assignment_target(&a.left);
                return;
            }
            Expression::UpdateExpression(u) => {
                match &u.argument {
                    oxc::ast::ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                        self.check_discard(&m.object);
                        self.check_discard(&m.expression);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                        self.check_discard(&m.object);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                        self.check_discard(&m.object);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::TSAsExpression(e) => {
                        self.check_discard(&e.expression);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => {
                        self.check_discard(&e.expression);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::TSNonNullExpression(e) => {
                        self.check_discard(&e.expression);
                    }
                    oxc::ast::ast::SimpleAssignmentTarget::TSTypeAssertion(e) => {
                        self.check_discard(&e.expression);
                    }
                    _ => {}
                }
                return;
            }
            Expression::StaticMemberExpression(m) => {
                self.check_discard(&m.object);
                return;
            }
            Expression::ComputedMemberExpression(m) => {
                self.check_discard(&m.object);
                self.check_discard(&m.expression);
                return;
            }
            Expression::PrivateFieldExpression(m) => {
                self.check_discard(&m.object);
                return;
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_discard(e);
                            }
                        }
                    }
                }
                return;
            }
            Expression::ObjectExpression(o) => {
                self.check_object(o);
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                            self.check_discard_prop_key(&p.key);
                            self.check_discard(&p.value);
                        }
                        oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                            self.check_discard(&s.argument);
                        }
                    }
                }
                return;
            }
            Expression::YieldExpression(y) => {
                if let Some(a) = &y.argument {
                    self.check_discard(a);
                }
                return;
            }
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(_) => {}
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    self.check_discard(&m.object);
                    return;
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.check_discard(&m.object);
                    self.check_discard(&m.expression);
                    return;
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    self.check_discard(&m.object);
                    return;
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.check_discard(&n.expression);
                    return;
                }
            },
            Expression::UnaryExpression(u) => {
                self.check_discard(&u.argument);
                return;
            }
            Expression::BinaryExpression(b) => {
                self.check_discard(&b.left);
                self.check_discard(&b.right);
                return;
            }
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.check_discard(e);
                }
                return;
            }
            Expression::TaggedTemplateExpression(t) => {
                self.check_tagged_template_parts(t);
            }
            Expression::V8IntrinsicExpression(v) => {
                for (i, a) in v.arguments.iter().enumerate() {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_unique_arg(e, None, i);
                            }
                        }
                    }
                }
                return;
            }
            Expression::NewExpression(n) => {
                if let Some(ta) = &n.type_arguments {
                    self.check_ts_type_args(ta);
                }
                self.check_discard(&n.callee);
            }
            Expression::ImportExpression(i) => {
                self.check_discard(&i.source);
                if let Some(o) = &i.options {
                    self.check_discard(o);
                }
                return;
            }
            Expression::FunctionExpression(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::ClassExpression(_) => {
                self.check_contained_fn(expr);
                return;
            }
            Expression::PrivateInExpression(p) => {
                self.check_discard(&p.right);
                return;
            }
            Expression::JSXElement(e) => {
                self.check_jsx_element(e);
                return;
            }
            Expression::JSXFragment(f) => {
                self.check_jsx_fragment(f);
                return;
            }
            Expression::AwaitExpression(a) => {
                self.check_discard(&a.argument);
                return;
            }
            Expression::ParenthesizedExpression(p) => {
                self.check_discard(&p.expression);
                return;
            }
            Expression::TSAsExpression(e) => {
                self.check_discard(&e.expression);
                self.check_ts_type(&e.type_annotation);
                return;
            }
            Expression::TSSatisfiesExpression(e) => {
                self.check_discard(&e.expression);
                self.check_ts_type(&e.type_annotation);
                return;
            }
            Expression::TSNonNullExpression(e) => {
                self.check_discard(&e.expression);
                return;
            }
            Expression::TSTypeAssertion(e) => {
                self.check_discard(&e.expression);
                self.check_ts_type(&e.type_annotation);
                return;
            }
            _ => {}
        }
        if self.features.move_tracking
            && self.features.exact_once
            && matches!(self.call_return_type(expr), Some(OwnType::Unique(_)))
        {
            let offset = expr.span().start;
            let msg = "unique value discarded without being bound or consumed";
            if !self
                .diags
                .iter()
                .any(|d| d.offset == offset && d.kind == RuleKind::UniqueForget && d.message == msg)
            {
                self.emit(offset, RuleKind::UniqueForget, msg);
            }
        }
        if let Some(call) = as_call(expr) {
            if let Some(ta) = &call.type_arguments {
                self.check_ts_type_args(ta);
            }
            self.check_call_callee(call);
            self.check_unique_rvalues(expr);
        } else if matches!(peel(expr), Expression::NewExpression(_)) {
            self.check_unique_rvalues(expr);
        }
    }

    fn check_call_callee(&mut self, call: &CallExpression<'_>) {
        if let Some((_, sig)) = self.instance_sig(call) {
            let mode = self.parameter_mode(sig.params.first().map(|(_, ty)| ty));
            if mode == ArgMode::Consume {
                if let Some(object) = instance_member_object(call) {
                    if ident_name(object).is_none() {
                        self.check_call_subexprs(object);
                        return;
                    }
                }
            }
        }
        self.check_discard(&call.callee);
    }

    /// Walk unique subexpressions of a bound unique/affine call without
    /// unique-forgetting the bound value itself.
    fn check_call_subexprs(&mut self, expr: &Expression<'_>) {
        self.walk_type_wrappers(expr);
        if let Expression::AssignmentExpression(a) = peel(expr) {
            self.check_discard_assignment_target(&a.left);
            self.check_call_subexprs(&a.right);
            return;
        }
        if let Some(call) = as_call(expr) {
            if let Some(ta) = &call.type_arguments {
                self.check_ts_type_args(ta);
            }
            self.check_call_callee(call);
            self.check_unique_rvalues(expr);
            return;
        }
        match peel(expr) {
            Expression::NewExpression(n) => {
                if let Some(ta) = &n.type_arguments {
                    self.check_ts_type_args(ta);
                }
                self.check_discard(&n.callee);
                self.check_unique_rvalues(expr);
            }
            Expression::TaggedTemplateExpression(t) => {
                self.check_tagged_template_parts(t);
            }
            Expression::SequenceExpression(s) => {
                let n = s.expressions.len();
                for (i, e) in s.expressions.iter().enumerate() {
                    if i + 1 < n {
                        self.check_discard(e);
                    } else {
                        self.check_call_subexprs(e);
                    }
                }
            }
            Expression::LogicalExpression(b) => match b.operator {
                LogicalOperator::And => {
                    self.check_discard(&b.left);
                    self.check_call_subexprs(&b.right);
                }
                LogicalOperator::Or | LogicalOperator::Coalesce => {
                    if matches!(self.call_return_type(&b.left), Some(OwnType::Unique(_))) {
                        self.check_call_subexprs(&b.left);
                        self.check_discard(&b.right);
                    } else {
                        self.check_discard(&b.left);
                        self.check_call_subexprs(&b.right);
                    }
                }
            },
            Expression::ConditionalExpression(c) => {
                self.check_discard(&c.test);
                self.check_call_subexprs(&c.consequent);
                self.check_call_subexprs(&c.alternate);
            }
            _ => {}
        }
    }

    fn walk_type_wrappers(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::ParenthesizedExpression(p) => self.walk_type_wrappers(&p.expression),
            Expression::AwaitExpression(a) => self.walk_type_wrappers(&a.argument),
            Expression::TSAsExpression(e) => {
                self.check_ts_type(&e.type_annotation);
                self.walk_type_wrappers(&e.expression);
            }
            Expression::TSSatisfiesExpression(e) => {
                self.check_ts_type(&e.type_annotation);
                self.walk_type_wrappers(&e.expression);
            }
            Expression::TSTypeAssertion(e) => {
                self.check_ts_type(&e.type_annotation);
                self.walk_type_wrappers(&e.expression);
            }
            Expression::TSNonNullExpression(e) => self.walk_type_wrappers(&e.expression),
            Expression::TSInstantiationExpression(e) => {
                self.check_ts_type_args(&e.type_arguments);
                self.walk_type_wrappers(&e.expression);
            }
            _ => {}
        }
    }

    fn check_discard_binding(&mut self, pat: &BindingPattern<'_>) {
        match pat {
            BindingPattern::AssignmentPattern(a) => {
                self.check_discard(&a.right);
                self.check_discard_binding(&a.left);
            }
            BindingPattern::ObjectPattern(o) => {
                for p in &o.properties {
                    self.check_discard_prop_key(&p.key);
                    self.check_discard_binding(&p.value);
                }
                if let Some(r) = &o.rest {
                    self.check_discard_binding(&r.argument);
                }
            }
            BindingPattern::ArrayPattern(a) => {
                for el in &a.elements {
                    if let Some(p) = el {
                        self.check_discard_binding(p);
                    }
                }
                if let Some(r) = &a.rest {
                    self.check_discard_binding(&r.argument);
                }
            }
            BindingPattern::BindingIdentifier(_) => {}
        }
    }

    fn check_discard_assignment_target(&mut self, t: &oxc::ast::ast::AssignmentTarget<'_>) {
        match t {
            oxc::ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                self.check_discard(&m.object);
                self.check_discard(&m.expression);
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                self.check_discard(&m.object);
            }
            oxc::ast::ast::AssignmentTarget::PrivateFieldExpression(m) => {
                self.check_discard(&m.object);
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.check_discard_atmd(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.check_discard_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                self.check_discard(init);
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.check_discard_prop_key(&p.name);
                            self.check_discard_atmd(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.check_discard_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => {
                self.check_discard(&e.expression);
            }
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                self.check_discard(&e.expression);
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                self.check_discard(&e.expression);
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => {
                self.check_discard(&e.expression);
            }
            _ => {}
        }
    }

    fn check_discard_atmd(&mut self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.check_discard(&d.init);
                self.check_discard_assignment_target(&d.binding);
            }
            oxc::ast::ast::AssignmentTargetMaybeDefault::ComputedMemberExpression(m) => {
                self.check_discard(&m.object);
                self.check_discard(&m.expression);
            }
            oxc::ast::ast::AssignmentTargetMaybeDefault::StaticMemberExpression(m) => {
                self.check_discard(&m.object);
            }
            oxc::ast::ast::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(a) => {
                for el in &a.elements {
                    if let Some(e) = el {
                        self.check_discard_atmd(e);
                    }
                }
                if let Some(r) = &a.rest {
                    self.check_discard_assignment_target(&r.target);
                }
            }
            oxc::ast::ast::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                self.check_discard(init);
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.check_discard_prop_key(&p.name);
                            self.check_discard_atmd(&p.binding);
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    self.check_discard_assignment_target(&r.target);
                }
            }
            _ => {
                if let Some(t) = t.as_assignment_target() {
                    self.check_discard_assignment_target(t);
                }
            }
        }
    }

    fn check_discard_prop_key(&mut self, key: &oxc::ast::ast::PropertyKey<'_>) {
        if let oxc::ast::ast::PropertyKey::StaticIdentifier(_)
        | oxc::ast::ast::PropertyKey::PrivateIdentifier(_) = key
        {
            return;
        }
        if let Some(e) = key.as_expression() {
            self.check_discard(e);
        }
    }

    fn discard_sequence_prefix(&mut self, expr: &Expression<'_>) {
        match peel(expr) {
            Expression::SequenceExpression(s) => {
                let n = s.expressions.len();
                for (i, e) in s.expressions.iter().enumerate() {
                    if i + 1 < n {
                        self.check_discard(e);
                    } else {
                        self.discard_sequence_prefix(e);
                    }
                }
            }
            Expression::LogicalExpression(b) if matches!(b.operator, LogicalOperator::And) => {
                self.check_discard(&b.left);
                self.discard_sequence_prefix(&b.right);
            }
            Expression::LogicalExpression(b)
                if matches!(b.operator, LogicalOperator::Or | LogicalOperator::Coalesce) =>
            {
                if matches!(self.call_return_type(&b.left), Some(OwnType::Unique(_)))
                    || ident_move_src(&b.left).is_some()
                {
                    self.discard_sequence_prefix(&b.left);
                    self.check_discard(&b.right);
                } else {
                    self.check_discard(&b.left);
                    self.discard_sequence_prefix(&b.right);
                }
            }
            Expression::ConditionalExpression(c) => {
                self.check_discard(&c.test);
                self.discard_sequence_prefix(&c.consequent);
                self.discard_sequence_prefix(&c.alternate);
            }
            Expression::AssignmentExpression(a) => {
                self.check_discard_assignment_target(&a.left);
                self.discard_sequence_prefix(&a.right);
            }
            _ => {}
        }
    }

    fn check_returned_unique(&mut self, expr: &Expression<'_>) {
        self.walk_type_wrappers(expr);
        let expr = peel(expr);
        match expr {
            Expression::SequenceExpression(s) => {
                let n = s.expressions.len();
                for (i, e) in s.expressions.iter().enumerate() {
                    if i + 1 == n {
                        self.check_returned_unique(e);
                    } else {
                        self.check_discard(e);
                    }
                }
                return;
            }
            Expression::AssignmentExpression(a) => {
                self.check_discard_assignment_target(&a.left);
                self.check_returned_unique(&a.right);
                return;
            }
            Expression::ConditionalExpression(c)
                if matches!(self.fn_ret, Some(OwnType::Unique(_))) =>
            {
                self.check_discard(&c.test);
                self.check_returned_unique(&c.consequent);
                self.check_returned_unique(&c.alternate);
                return;
            }
            Expression::LogicalExpression(b) if matches!(self.fn_ret, Some(OwnType::Unique(_))) => {
                match b.operator {
                    LogicalOperator::And => {
                        self.check_discard(&b.left);
                        self.check_returned_unique(&b.right);
                    }
                    LogicalOperator::Or | LogicalOperator::Coalesce => {
                        if matches!(self.call_return_type(&b.left), Some(OwnType::Unique(_))) {
                            self.check_returned_unique(&b.left);
                            self.check_discard(&b.right);
                        } else {
                            self.check_discard(&b.left);
                            self.check_returned_unique(&b.right);
                        }
                    }
                }
                return;
            }
            _ => {}
        }
        if matches!(self.call_return_type(expr), Some(OwnType::Unique(_)))
            && matches!(self.fn_ret, Some(OwnType::Unique(_)))
        {
            return;
        }
        self.check_discard(expr);
    }

    fn call_return_type(&self, expr: &Expression<'_>) -> Option<OwnType> {
        if !self.features.owned_return_propagation {
            return None;
        }
        let expr = peel(expr);
        match expr {
            Expression::SequenceExpression(s) => {
                return s.expressions.last().and_then(|e| self.call_return_type(e));
            }
            Expression::AssignmentExpression(a) => {
                return self.call_return_type(&a.right);
            }
            Expression::ConditionalExpression(c) => {
                let a = self.call_return_type(&c.consequent);
                let b = self.call_return_type(&c.alternate);
                return match (a, b) {
                    (Some(OwnType::Unique(x)), Some(OwnType::Unique(_))) => {
                        Some(OwnType::Unique(x))
                    }
                    _ => None,
                };
            }
            Expression::LogicalExpression(b) => {
                return match b.operator {
                    LogicalOperator::And => self.call_return_type(&b.right),
                    LogicalOperator::Or | LogicalOperator::Coalesce => {
                        match (
                            self.call_return_type(&b.left),
                            self.call_return_type(&b.right),
                        ) {
                            (Some(OwnType::Unique(x)), _) => Some(OwnType::Unique(x)),
                            (_, Some(OwnType::Unique(y))) => Some(OwnType::Unique(y)),
                            (a, _) => a,
                        }
                    }
                };
            }
            Expression::UnaryExpression(u) if matches!(u.operator, UnaryOperator::Void) => {
                return None;
            }
            Expression::TaggedTemplateExpression(t) => {
                if let Some(name) = callee_name(&t.tag) {
                    return self.callee_sig(&name).map(|s| s.ret.clone());
                }
            }
            _ => {}
        }
        if let Expression::NewExpression(n) = peel(expr) {
            if let Some(name) = callee_name(&n.callee) {
                return self.callee_sig(&name).map(|s| s.ret.clone());
            }
        }
        let call = as_call(expr)?;
        if let Some((_, sig)) = self.instance_sig(call) {
            return Some(sig.ret.clone());
        }
        if let Some(name) = callee_name(&call.callee) {
            if let Some(s) = self.callee_sig(&name) {
                return Some(s.ret.clone());
            }
        }
        match peel(&call.callee) {
            Expression::FunctionExpression(f) => {
                let mut offs = vec![f.span.start];
                if let Some(id) = &f.id {
                    offs.push(id.span.start);
                }
                return self.file.type_sig_at(&offs).map(|s| s.ret.clone());
            }
            Expression::ArrowFunctionExpression(a) => {
                return self
                    .file
                    .type_sig_at(&[a.span.start])
                    .map(|s| s.ret.clone());
            }
            _ => None,
        }
    }

    /// `buf.toString()` → prelude key `Buffer#toString` using the receiver's own type.
    fn instance_sig(&self, call: &CallExpression<'_>) -> Option<(String, FnSig)> {
        if !self.features.instance_dispatch {
            return None;
        }
        let (object, method) = match &call.callee {
            Expression::StaticMemberExpression(m) => {
                (&m.object, m.property.name.as_str().to_string())
            }
            Expression::ComputedMemberExpression(m) => (&m.object, string_prop_key(&m.expression)?),
            _ => return None,
        };
        let (recv, ty) = if let Some(recv) = ident_name(object).or_else(|| ident_move_src(object)) {
            (recv.clone(), self.receiver_type_name(&recv)?)
        } else if let Some(cname) = new_instance_type(object) {
            (String::new(), cname)
        } else {
            let ret = self.call_return_type(object)?;
            let ty = ret.type_name();
            if ty.is_empty() || ty == "void" {
                return None;
            }
            (String::new(), ty.to_string())
        };
        let key = format!("{ty}#{method}");
        let sig = self.callee_sig(&key)?.clone();
        Some((recv, sig))
    }

    fn receiver_type_name(&self, ident: &str) -> Option<String> {
        if let Some(e) = self.tbl.get(ident) {
            if !e.ty_name.is_empty() {
                return Some(e.ty_name.clone());
            }
        }
        None
    }

    fn check_unmapped_eval(&mut self, expr: &Expression<'_>) {
        if let Expression::CallExpression(call) = expr {
            if ident_name(&call.callee).as_deref() == Some("eval") {
                self.emit(
                    call.span.start,
                    RuleKind::UnmappedConstruct,
                    "`eval` is not mapped from Austral linearity",
                );
            }
        }
    }

    fn scan_unmapped_body(&mut self, body: &FunctionBody<'_>) {
        for s in &body.statements {
            if let Statement::WithStatement(w) = s {
                self.emit(
                    w.span.start,
                    RuleKind::UnmappedConstruct,
                    "`with` statements are not mapped from Austral linearity",
                );
            }
            if let Statement::ExpressionStatement(e) = s {
                self.check_unmapped_eval(&e.expression);
            }
        }
    }

    fn check_expr(&mut self, expr: &Expression<'_>, span: u32) {
        self.check_unmapped_in_expr(expr);
        self.check_nested_fn_captures(expr);
        match expr {
            Expression::LogicalExpression(b) => {
                self.check_expr(&b.left, b.left.span().start);
                let saved = self.tbl.clone();
                self.check_expr(&b.right, b.right.span().start);
                let after_right = self.tbl.clone();
                self.tables_consistent("a logical expression", &saved, &after_right, span);
                self.tbl = after_right;
                return;
            }
            Expression::ConditionalExpression(c) => {
                self.check_expr(&c.test, c.test.span().start);
                let saved = self.tbl.clone();
                self.check_expr(&c.consequent, c.consequent.span().start);
                if !self.features.control_flow_splitting {
                    self.check_expr(&c.alternate, c.alternate.span().start);
                    return;
                }
                let then_tbl = self.tbl.clone();
                self.tbl = saved;
                self.check_expr(&c.alternate, c.alternate.span().start);
                let else_tbl = self.tbl.clone();
                self.tables_consistent("a conditional expression", &then_tbl, &else_tbl, span);
                self.tbl = then_tbl;
                return;
            }
            _ => {}
        }
        let names: Vec<String> = self.tbl.keys().cloned().collect();
        for name in names {
            let apps = self.count(expr, &name);
            if !apps.is_zero() {
                self.apply_apps(&name, apps, span);
            }
        }
        if let Expression::AssignmentExpression(a) = expr {
            if let Some(rhs) = ident_name(&a.right) {
                if let Some(e) = self.tbl.get(&rhs) {
                    if matches!(e.kind, OwnKind::RefRead | OwnKind::RefWrite) {
                        self.emit(
                            a.span.start,
                            RuleKind::BorrowEscape,
                            format!("borrow `{rhs}` escapes its lexical region via assignment"),
                        );
                    }
                }
            }
        }
        self.check_unique_rvalues(expr);
        self.visit_exclusive_nested(expr);
        self.check_contained_fn(expr);
    }

    /// Plan ownership transfer for a discarded, simple identifier assignment.
    ///
    /// The assignment expression's value is deliberately restricted to an
    /// expression statement: in value-producing contexts, another binding or
    /// call may immediately take ownership of that same value.
    fn discarded_assignment_transfer(
        &self,
        expr: &Expression<'_>,
    ) -> Option<(String, String, VarEntry)> {
        let Expression::AssignmentExpression(assignment) = peel(expr) else {
            return None;
        };
        if assignment.operator != AssignmentOperator::Assign {
            return None;
        }
        let oxc::ast::ast::AssignmentTarget::AssignmentTargetIdentifier(destination) =
            &assignment.left
        else {
            return None;
        };
        let source = ident_move_src(&assignment.right)?;
        let entry = self.tbl.get(&source)?.clone();
        if entry.state != VarState::Unconsumed
            || !matches!(entry.kind, OwnKind::Unique | OwnKind::Affine)
        {
            return None;
        }
        let destination = destination.name.as_str().to_string();
        if source == destination {
            let apps = self.count(&assignment.right, &source);
            if apps.consumed != 1 || apps.read != 0 || apps.write != 0 || apps.path != 0 {
                // Suppressing the assignment's self-move is only sound when
                // it is the RHS's sole use of the owned value.
                return None;
            }
        }
        // An untracked target's declaration scope is unavailable in this
        // name-keyed checker. Only introduce one in the current function or
        // program scope; tracked targets retain their existing scope slot.
        if !self.tbl.contains_key(&destination) && self.scopes.len() != 1 {
            return None;
        }
        Some((source, destination, entry))
    }

    fn finish_discarded_assignment_transfer(
        &mut self,
        source: &str,
        destination: &str,
        mut entry: VarEntry,
        span: u32,
    ) {
        if !matches!(self.tbl.get(source), Some(e) if e.state == VarState::Consumed) {
            // The RHS was rejected (for example, a double move, active borrow,
            // or cross-loop move), so it cannot establish a new owner.
            return;
        }
        entry.state = VarState::Unconsumed;
        entry.loop_depth = self.loop_depth;
        entry.defined_at = span;
        entry.owner = None;
        entry.read_borrows = 0;
        entry.write_borrows = 0;
        if self.tbl.contains_key(destination) {
            // Settle the overwritten value, then keep the destination's
            // existing lexical scope record for the replacement.
            self.remove_var(destination);
            self.tbl.insert(destination.to_string(), entry);
        } else {
            self.add_var(destination.to_string(), entry);
        }
    }

    fn check_unmapped_in_expr(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::ComputedMemberExpression(m) => {
                if let Some(n) = ident_name(&m.object) {
                    if self.tbl.contains_key(&n) {
                        self.emit(
                            m.span.start,
                            RuleKind::UnmappedConstruct,
                            format!("computed property access on owned value `{n}` is not mapped"),
                        );
                    }
                }
            }
            Expression::StaticMemberExpression(m) => {
                if m.property.name.as_str() == "__proto__"
                    || m.property.name.as_str() == "prototype"
                {
                    if let Some(n) = ident_name(&m.object) {
                        if self.tbl.contains_key(&n) {
                            self.emit(
                                m.span.start,
                                RuleKind::UnmappedConstruct,
                                format!("prototype access on owned value `{n}` is not mapped"),
                            );
                        }
                    }
                }
            }
            Expression::CallExpression(c) => {
                if ident_name(&c.callee).as_deref() == Some("eval") {
                    self.emit(
                        c.span.start,
                        RuleKind::UnmappedConstruct,
                        "`eval` is not mapped from Austral linearity",
                    );
                }
                if let Expression::ComputedMemberExpression(m) = &c.callee {
                    if let Some(n) = ident_name(&m.object) {
                        if self.tbl.contains_key(&n) {
                            self.emit(
                                m.span.start,
                                RuleKind::UnmappedConstruct,
                                format!(
                                    "computed property access on owned value `{n}` is not mapped"
                                ),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn check_nested_fn_captures(&mut self, expr: &Expression<'_>) {
        let owned: Vec<String> = self.tbl.keys().cloned().collect();
        if owned.is_empty() {
            return;
        }
        let expr = peel(expr);
        match expr {
            Expression::FunctionExpression(f) => {
                self.check_param_default_captures(f);
                if let Some(body) = &f.body {
                    let owned = capture_candidates_for_function(owned, f);
                    self.report_captures_in_body(body, &owned);
                }
            }
            Expression::ArrowFunctionExpression(a) => {
                self.report_captures_arrow(a);
            }
            Expression::CallExpression(c) => {
                self.check_nested_fn_captures(&c.callee);
                for a in &c.arguments {
                    if let Some(e) = a.as_expression() {
                        self.check_nested_fn_captures(e);
                    }
                }
            }
            Expression::NewExpression(n) => {
                self.check_nested_fn_captures(&n.callee);
                for a in &n.arguments {
                    if let Some(e) = a.as_expression() {
                        self.check_nested_fn_captures(e);
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    if let Some(e) = el.as_expression() {
                        self.check_nested_fn_captures(e);
                    }
                }
            }
            Expression::ParenthesizedExpression(p) => self.check_nested_fn_captures(&p.expression),
            Expression::AwaitExpression(a) => self.check_nested_fn_captures(&a.argument),
            Expression::UnaryExpression(u) => self.check_nested_fn_captures(&u.argument),
            Expression::AssignmentExpression(a) => self.check_nested_fn_captures(&a.right),
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.check_nested_fn_captures(e);
                }
            }
            Expression::ObjectExpression(o) => {
                for p in &o.properties {
                    if let oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) = p {
                        self.check_nested_fn_captures(&p.value);
                    }
                }
            }
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.check_nested_fn_captures(e);
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                self.check_nested_fn_captures(&t.tag);
                for e in &t.quasi.expressions {
                    self.check_nested_fn_captures(e);
                }
            }
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => {
                    self.check_nested_fn_captures(&call.callee);
                    for a in &call.arguments {
                        if let Some(e) = a.as_expression() {
                            self.check_nested_fn_captures(e);
                        }
                    }
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    self.check_nested_fn_captures(&m.object)
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.check_nested_fn_captures(&m.object);
                    self.check_nested_fn_captures(&m.expression);
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    self.check_nested_fn_captures(&m.object)
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.check_nested_fn_captures(&n.expression)
                }
            },
            Expression::LogicalExpression(b) => {
                self.check_nested_fn_captures(&b.left);
                self.check_nested_fn_captures(&b.right);
            }
            Expression::ConditionalExpression(c) => {
                self.check_nested_fn_captures(&c.test);
                self.check_nested_fn_captures(&c.consequent);
                self.check_nested_fn_captures(&c.alternate);
            }
            _ => {}
        }
    }

    fn report_captures_in_body(&mut self, body: &FunctionBody<'_>, owned: &[String]) {
        let owned = capture_candidates_after_hoisted_vars(owned, &body.statements);
        self.report_captures_statements(&body.statements, &owned);
    }

    fn report_captures_statements(&mut self, statements: &[Statement<'_>], owned: &[String]) {
        let owned = capture_candidates_after_local_declarations(owned, statements);
        if owned.is_empty() {
            return;
        }
        for stmt in statements {
            self.report_captures_stmt(stmt, &owned);
        }
    }

    fn report_captures_stmt(&mut self, stmt: &Statement<'_>, owned: &[String]) {
        match stmt {
            Statement::ExpressionStatement(e) => self.report_captures_expr(&e.expression, owned),
            Statement::ReturnStatement(r) => {
                if let Some(a) = &r.argument {
                    self.report_captures_expr(a, owned);
                }
            }
            Statement::ThrowStatement(t) => self.report_captures_expr(&t.argument, owned),
            Statement::BlockStatement(b) => {
                self.report_captures_statements(&b.body, owned);
            }
            Statement::IfStatement(i) => {
                self.report_captures_expr(&i.test, owned);
                self.report_captures_stmt(&i.consequent, owned);
                if let Some(a) = &i.alternate {
                    self.report_captures_stmt(a, owned);
                }
            }
            Statement::WhileStatement(w) => {
                self.report_captures_expr(&w.test, owned);
                self.report_captures_stmt(&w.body, owned);
            }
            Statement::DoWhileStatement(w) => {
                self.report_captures_stmt(&w.body, owned);
                self.report_captures_expr(&w.test, owned);
            }
            Statement::TryStatement(t) => {
                self.report_captures_statements(&t.block.body, owned);
                if let Some(h) = &t.handler {
                    let mut catch_owned = owned.to_vec();
                    if let Some(param) = &h.param {
                        let mut bound = HashSet::new();
                        collect_binding_names(&param.pattern, &mut bound);
                        catch_owned.retain(|name| !bound.contains(name));
                    }
                    self.report_captures_statements(&h.body.body, &catch_owned);
                }
                if let Some(f) = &t.finalizer {
                    self.report_captures_statements(&f.body, owned);
                }
            }
            Statement::VariableDeclaration(v) => {
                for d in &v.declarations {
                    if let Some(i) = &d.init {
                        self.report_captures_expr(i, owned);
                    }
                }
            }
            Statement::ForStatement(f) => {
                if let Some(init) = &f.init {
                    match init {
                        ForStatementInit::VariableDeclaration(v) => {
                            for d in &v.declarations {
                                if let Some(i) = &d.init {
                                    self.report_captures_expr(i, owned);
                                }
                            }
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.report_captures_expr(e, owned);
                            }
                        }
                    }
                }
                if let Some(test) = &f.test {
                    self.report_captures_expr(test, owned);
                }
                if let Some(upd) = &f.update {
                    self.report_captures_expr(upd, owned);
                }
                self.report_captures_stmt(&f.body, owned);
            }
            Statement::ForInStatement(f) => {
                self.report_captures_expr(&f.right, owned);
                self.report_captures_stmt(&f.body, owned);
            }
            Statement::ForOfStatement(f) => {
                self.report_captures_expr(&f.right, owned);
                self.report_captures_stmt(&f.body, owned);
            }
            Statement::SwitchStatement(s) => {
                self.report_captures_expr(&s.discriminant, owned);
                for case in &s.cases {
                    for st in &case.consequent {
                        self.report_captures_stmt(st, owned);
                    }
                }
            }
            Statement::FunctionDeclaration(f) => {
                let nested_owned = capture_candidates_for_function(owned.to_vec(), f);
                for p in &f.params.items {
                    if let Some(init) = &p.initializer {
                        self.report_captures_expr(init, &nested_owned);
                    }
                }
                if let Some(body) = &f.body {
                    self.report_captures_in_body(body, &nested_owned);
                }
            }
            Statement::LabeledStatement(l) => self.report_captures_stmt(&l.body, owned),
            Statement::ClassDeclaration(c) => self.report_captures_class(c, owned),
            _ => {}
        }
    }

    fn report_captures_class(&mut self, class: &oxc::ast::ast::Class<'_>, owned: &[String]) {
        for el in &class.body.body {
            match el {
                oxc::ast::ast::ClassElement::MethodDefinition(m) => {
                    for p in &m.value.params.items {
                        if let Some(init) = &p.initializer {
                            self.report_captures_expr(init, owned);
                        }
                    }
                    if let Some(body) = &m.value.body {
                        for s in &body.statements {
                            self.report_captures_stmt(s, owned);
                        }
                    }
                }
                oxc::ast::ast::ClassElement::PropertyDefinition(p) => {
                    if let Some(v) = &p.value {
                        self.report_captures_expr(v, owned);
                    }
                }
                _ => {}
            }
        }
    }

    fn report_captures_expr(&mut self, expr: &Expression<'_>, owned: &[String]) {
        if let Some(n) = ident_name(expr) {
            if owned.iter().any(|o| o == &n) {
                let offset = expr.span().start;
                let msg = format!("nested function capturing owned value `{n}` is not mapped");
                if !self.diags.iter().any(|d| {
                    d.offset == offset && d.kind == RuleKind::UnmappedConstruct && d.message == msg
                }) {
                    self.emit(offset, RuleKind::UnmappedConstruct, msg);
                }
            }
        }
        match expr {
            Expression::CallExpression(c) => {
                self.report_captures_expr(&c.callee, owned);
                for a in &c.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.report_captures_expr(&s.argument, owned)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.report_captures_expr(e, owned);
                            }
                        }
                    }
                }
            }
            Expression::NewExpression(n) => {
                self.report_captures_expr(&n.callee, owned);
                for a in &n.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.report_captures_expr(&s.argument, owned)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.report_captures_expr(e, owned);
                            }
                        }
                    }
                }
            }
            Expression::StaticMemberExpression(m) => self.report_captures_expr(&m.object, owned),
            Expression::ComputedMemberExpression(m) => {
                self.report_captures_expr(&m.object, owned);
                self.report_captures_expr(&m.expression, owned);
            }
            Expression::BinaryExpression(b) => {
                self.report_captures_expr(&b.left, owned);
                self.report_captures_expr(&b.right, owned);
            }
            Expression::LogicalExpression(b) => {
                self.report_captures_expr(&b.left, owned);
                self.report_captures_expr(&b.right, owned);
            }
            Expression::ConditionalExpression(c) => {
                self.report_captures_expr(&c.test, owned);
                self.report_captures_expr(&c.consequent, owned);
                self.report_captures_expr(&c.alternate, owned);
            }
            Expression::UnaryExpression(u) => self.report_captures_expr(&u.argument, owned),
            Expression::AwaitExpression(a) => self.report_captures_expr(&a.argument, owned),
            Expression::ParenthesizedExpression(p) => {
                self.report_captures_expr(&p.expression, owned)
            }
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.report_captures_expr(e, owned);
                }
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.report_captures_expr(&s.argument, owned)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.report_captures_expr(e, owned);
                            }
                        }
                    }
                }
            }
            Expression::ObjectExpression(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                            if let Some(k) = p.key.as_expression() {
                                self.report_captures_expr(k, owned);
                            }
                            self.report_captures_expr(&p.value, owned);
                        }
                        oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                            self.report_captures_expr(&s.argument, owned)
                        }
                    }
                }
            }
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.report_captures_expr(e, owned);
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                self.report_captures_expr(&t.tag, owned);
                for e in &t.quasi.expressions {
                    self.report_captures_expr(e, owned);
                }
            }
            Expression::AssignmentExpression(a) => self.report_captures_expr(&a.right, owned),
            Expression::FunctionExpression(f) => {
                let nested_owned = capture_candidates_for_function(owned.to_vec(), f);
                for p in &f.params.items {
                    if let Some(init) = &p.initializer {
                        self.report_captures_expr(init, &nested_owned);
                    }
                }
                if let Some(body) = &f.body {
                    self.report_captures_in_body(body, &nested_owned);
                }
            }
            Expression::ArrowFunctionExpression(a) => {
                let nested_owned = capture_candidates_for_params(owned.to_vec(), &a.params, None);
                for p in &a.params.items {
                    if let Some(init) = &p.initializer {
                        self.report_captures_expr(init, &nested_owned);
                    }
                }
                match &a.body {
                    ArrowFunctionBody::FunctionBody(body) => {
                        self.report_captures_in_body(body, &nested_owned);
                    }
                    other => {
                        if let Some(e) = other.as_expression() {
                            self.report_captures_expr(e, &nested_owned);
                        }
                    }
                }
            }
            Expression::ClassExpression(c) => self.report_captures_class(c, owned),
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => {
                    self.report_captures_expr(&call.callee, owned);
                    for a in &call.arguments {
                        if let Some(e) = a.as_expression() {
                            self.report_captures_expr(e, owned);
                        }
                    }
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    self.report_captures_expr(&m.object, owned)
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.report_captures_expr(&m.object, owned);
                    self.report_captures_expr(&m.expression, owned);
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    self.report_captures_expr(&m.object, owned)
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.report_captures_expr(&n.expression, owned)
                }
            },
            Expression::JSXElement(e) => self.report_captures_jsx_element(e, owned),
            Expression::JSXFragment(f) => self.report_captures_jsx_fragment(f, owned),
            _ => {}
        }
    }

    fn report_captures_jsx_element(
        &mut self,
        el: &oxc::ast::ast::JSXElement<'_>,
        owned: &[String],
    ) {
        for attr in &el.opening_element.attributes {
            match attr {
                oxc::ast::ast::JSXAttributeItem::Attribute(a) => {
                    if let Some(oxc::ast::ast::JSXAttributeValue::ExpressionContainer(e)) = &a.value
                    {
                        if let Some(x) = e.expression.as_expression() {
                            self.report_captures_expr(x, owned);
                        }
                    }
                }
                oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                    self.report_captures_expr(&s.argument, owned)
                }
            }
        }
        for c in &el.children {
            self.report_captures_jsx_child(c, owned);
        }
    }

    fn report_captures_jsx_fragment(
        &mut self,
        f: &oxc::ast::ast::JSXFragment<'_>,
        owned: &[String],
    ) {
        for c in &f.children {
            self.report_captures_jsx_child(c, owned);
        }
    }

    fn report_captures_jsx_child(&mut self, c: &oxc::ast::ast::JSXChild<'_>, owned: &[String]) {
        match c {
            oxc::ast::ast::JSXChild::Element(e) => self.report_captures_jsx_element(e, owned),
            oxc::ast::ast::JSXChild::Fragment(f) => self.report_captures_jsx_fragment(f, owned),
            oxc::ast::ast::JSXChild::ExpressionContainer(e) => {
                if let Some(x) = e.expression.as_expression() {
                    self.report_captures_expr(x, owned);
                }
            }
            oxc::ast::ast::JSXChild::Spread(s) => self.report_captures_expr(&s.expression, owned),
            oxc::ast::ast::JSXChild::Text(_) => {}
        }
    }

    fn count(&self, expr: &Expression<'_>, name: &str) -> Apps {
        match expr {
            Expression::Identifier(id) => {
                if id.name.as_str() == name {
                    if self.suppress_consume.as_deref() == Some(name) {
                        return Apps::default();
                    }
                    return Apps {
                        consumed: 1,
                        ..Apps::default()
                    };
                }
                Apps::default()
            }
            Expression::CallExpression(call) => self.count_call(call, name),
            Expression::NewExpression(n) => {
                let mut apps = self.count(&n.callee, name);
                let callee = callee_name(&n.callee);
                let sig = callee.as_ref().and_then(|n| self.callee_sig(n));
                for (i, arg) in n.arguments.iter().enumerate() {
                    match arg {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            let spread =
                                if sig.is_none() && !self.features.unknown_call_conservatism {
                                    self.as_non_consuming(self.count(&s.argument, name))
                                } else {
                                    self.count(&s.argument, name)
                                };
                            apps = apps.merge(spread);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                let mode = self.arg_mode(e, sig, i);
                                apps = apps.merge(self.count_arg(e, name, mode));
                            }
                        }
                    }
                }
                apps
            }
            Expression::StaticMemberExpression(m) => {
                let head = if ident_name(&m.object).as_deref() == Some(name) {
                    Apps {
                        path: 1,
                        ..Apps::default()
                    }
                } else {
                    self.count(&m.object, name)
                };
                head
            }
            Expression::PrivateFieldExpression(m) => {
                if ident_name(&m.object).as_deref() == Some(name) {
                    Apps {
                        path: 1,
                        ..Apps::default()
                    }
                } else {
                    self.count(&m.object, name)
                }
            }
            Expression::ComputedMemberExpression(m) => {
                let head = if ident_name(&m.object).as_deref() == Some(name) {
                    Apps {
                        path: 1,
                        ..Apps::default()
                    }
                } else {
                    self.count(&m.object, name)
                };
                head.merge(self.count(&m.expression, name))
            }
            Expression::AssignmentExpression(a) => self
                .count(&a.right, name)
                .merge(self.count_assignment_target(&a.left, name)),
            Expression::UnaryExpression(u)
                if matches!(u.operator, UnaryOperator::Void)
                    && self
                        .tbl
                        .get(name)
                        .is_some_and(|entry| entry.opaque_aggregate) =>
            {
                self.as_non_consuming(self.count(&u.argument, name))
            }
            Expression::UnaryExpression(u) => self.count(&u.argument, name),
            Expression::PrivateInExpression(p) => self.count(&p.right, name),
            Expression::UpdateExpression(u) => match &u.argument {
                oxc::ast::ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                    let head = if ident_name(&m.object).as_deref() == Some(name) {
                        Apps {
                            path: 1,
                            ..Apps::default()
                        }
                    } else {
                        self.count(&m.object, name)
                    };
                    head.merge(self.count(&m.expression, name))
                }
                oxc::ast::ast::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                    if ident_name(&m.object).as_deref() == Some(name) {
                        Apps {
                            path: 1,
                            ..Apps::default()
                        }
                    } else {
                        self.count(&m.object, name)
                    }
                }
                oxc::ast::ast::SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                    self.count(&m.object, name)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSAsExpression(e) => {
                    self.count(&e.expression, name)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => {
                    self.count(&e.expression, name)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSNonNullExpression(e) => {
                    self.count(&e.expression, name)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSTypeAssertion(e) => {
                    self.count(&e.expression, name)
                }
                _ => {
                    if let Some(id) = u.argument.get_identifier_name() {
                        if id == name {
                            return Apps {
                                consumed: 1,
                                ..Apps::default()
                            };
                        }
                    }
                    Apps::default()
                }
            },
            Expression::BinaryExpression(b) => {
                self.count(&b.left, name).merge(self.count(&b.right, name))
            }
            Expression::LogicalExpression(_) | Expression::ConditionalExpression(_) => {
                Apps::default()
            }
            Expression::SequenceExpression(s) => {
                let mut a = Apps::default();
                for e in &s.expressions {
                    a = a.merge(self.count(e, name));
                }
                a
            }
            Expression::ArrayExpression(arr) => {
                let mut a = Apps::default();
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            a = a.merge(self.count(&s.argument, name));
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                a = a.merge(self.count(e, name));
                            }
                        }
                    }
                }
                a
            }
            Expression::ObjectExpression(obj) => {
                let mut a = Apps::default();
                for p in &obj.properties {
                    match p {
                        oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                            if let Some(k) = p.key.as_expression() {
                                a = a.merge(self.count(k, name));
                            }
                            a = a.merge(self.count(&p.value, name));
                        }
                        oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                            a = a.merge(self.count(&s.argument, name));
                        }
                    }
                }
                a
            }
            Expression::ParenthesizedExpression(p) => self.count(&p.expression, name),
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => {
                    if !self.features.optional_call_paths
                        || self.optional_call_has_definite_callee(call)
                    {
                        return self.count_call(call, name);
                    }
                    // Optional calls may not run; do not definite-consume this/args.
                    let mut a = Apps::default();
                    if let Expression::StaticMemberExpression(m) = &call.callee {
                        if ident_name(&m.object).as_deref() == Some(name) {
                            a.path += 1;
                        } else {
                            a = a.merge(self.count(&m.object, name));
                        }
                    } else {
                        a = a.merge(self.count(&call.callee, name));
                    }
                    for arg in &call.arguments {
                        if let Some(e) = arg.as_expression() {
                            if ident_name(e).as_deref() == Some(name) {
                                a.path += 1;
                            } else {
                                a = a.merge(self.count(e, name));
                            }
                        }
                    }
                    a
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    let head = if ident_name(&m.object).as_deref() == Some(name) {
                        Apps {
                            path: 1,
                            ..Apps::default()
                        }
                    } else {
                        self.count(&m.object, name)
                    };
                    head.merge(self.count(&m.expression, name))
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    if ident_name(&m.object).as_deref() == Some(name) {
                        Apps {
                            path: 1,
                            ..Apps::default()
                        }
                    } else {
                        self.count(&m.object, name)
                    }
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    if ident_name(&m.object).as_deref() == Some(name) {
                        Apps {
                            path: 1,
                            ..Apps::default()
                        }
                    } else {
                        self.count(&m.object, name)
                    }
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.count(&n.expression, name)
                }
            },
            Expression::AwaitExpression(a) => self.count(&a.argument, name),
            Expression::YieldExpression(y) => y
                .argument
                .as_ref()
                .map(|e| self.count(e, name))
                .unwrap_or_default(),
            Expression::TemplateLiteral(t) => {
                let mut a = Apps::default();
                for e in &t.expressions {
                    a = a.merge(self.count(e, name));
                }
                a
            }
            Expression::TaggedTemplateExpression(t) => {
                let mut a = self.count(&t.tag, name);
                let callee = callee_name(&t.tag);
                let sig = callee.as_ref().and_then(|n| self.callee_sig(n));
                for (i, e) in t.quasi.expressions.iter().enumerate() {
                    let mode = self.arg_mode(e, sig, i + 1);
                    a = a.merge(self.count_arg(e, name, mode));
                }
                a
            }
            Expression::TSAsExpression(e) => self.count(&e.expression, name),
            Expression::TSSatisfiesExpression(e) => self.count(&e.expression, name),
            Expression::TSNonNullExpression(e) => self.count(&e.expression, name),
            Expression::TSTypeAssertion(e) => self.count(&e.expression, name),
            Expression::TSInstantiationExpression(e) => self.count(&e.expression, name),
            Expression::JSXElement(e) => self.count_jsx_element(e, name),
            Expression::JSXFragment(f) => self.count_jsx_fragment(f, name),
            Expression::ImportExpression(i) => {
                let mut a = self.count(&i.source, name);
                if let Some(o) = &i.options {
                    a = a.merge(self.count(o, name));
                }
                a
            }
            Expression::V8IntrinsicExpression(v) => {
                let mut a = Apps::default();
                for arg in &v.arguments {
                    match arg {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            a = a.merge(self.count(&s.argument, name));
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                a = a.merge(self.count_arg(e, name, ArgMode::Consume));
                            }
                        }
                    }
                }
                a
            }
            _ => Apps::default(),
        }
    }

    fn optional_call_has_definite_callee(&self, call: &CallExpression<'_>) -> bool {
        !callee_has_optional_access(&call.callee)
            && callee_name(&call.callee).is_some_and(|name| self.callee_sig(&name).is_some())
    }

    fn count_assignment_target(&self, t: &oxc::ast::ast::AssignmentTarget<'_>, name: &str) -> Apps {
        match t {
            oxc::ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                let head = if ident_name(&m.object).as_deref() == Some(name) {
                    Apps {
                        path: 1,
                        ..Apps::default()
                    }
                } else {
                    self.count(&m.object, name)
                };
                head.merge(self.count(&m.expression, name))
            }
            oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                if ident_name(&m.object).as_deref() == Some(name) {
                    Apps {
                        path: 1,
                        ..Apps::default()
                    }
                } else {
                    self.count(&m.object, name)
                }
            }
            oxc::ast::ast::AssignmentTarget::PrivateFieldExpression(m) => {
                self.count(&m.object, name)
            }
            oxc::ast::ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                let mut apps = Apps::default();
                for el in &a.elements {
                    if let Some(e) = el {
                        apps = apps.merge(self.count_atmd(e, name));
                    }
                }
                if let Some(r) = &a.rest {
                    apps = apps.merge(self.count_assignment_target(&r.target, name));
                }
                apps
            }
            oxc::ast::ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                let mut apps = Apps::default();
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            if let Some(init) = &p.init {
                                apps = apps.merge(self.count(init, name));
                            }
                        }
                        oxc::ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            if let Some(k) = p.name.as_expression() {
                                apps = apps.merge(self.count(k, name));
                            }
                            apps = apps.merge(self.count_atmd(&p.binding, name));
                        }
                    }
                }
                if let Some(r) = &o.rest {
                    apps = apps.merge(self.count_assignment_target(&r.target, name));
                }
                apps
            }
            oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => self.count(&e.expression, name),
            oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => {
                self.count(&e.expression, name)
            }
            oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => {
                self.count(&e.expression, name)
            }
            oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => self.count(&e.expression, name),
            _ => Apps::default(),
        }
    }

    fn count_atmd(&self, t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>, name: &str) -> Apps {
        match t {
            oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => self
                .count(&d.init, name)
                .merge(self.count_assignment_target(&d.binding, name)),
            _ => t
                .as_assignment_target()
                .map(|x| self.count_assignment_target(x, name))
                .unwrap_or_default(),
        }
    }

    fn count_jsx_element(&self, el: &oxc::ast::ast::JSXElement<'_>, name: &str) -> Apps {
        let mut a = Apps::default();
        for attr in &el.opening_element.attributes {
            match attr {
                oxc::ast::ast::JSXAttributeItem::Attribute(at) => {
                    if let Some(v) = &at.value {
                        a = a.merge(self.count_jsx_attr_value(v, name));
                    }
                }
                oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                    a = a.merge(self.count(&s.argument, name));
                }
            }
        }
        for c in &el.children {
            a = a.merge(self.count_jsx_child(c, name));
        }
        a
    }

    fn count_jsx_fragment(&self, f: &oxc::ast::ast::JSXFragment<'_>, name: &str) -> Apps {
        let mut a = Apps::default();
        for c in &f.children {
            a = a.merge(self.count_jsx_child(c, name));
        }
        a
    }

    fn count_jsx_child(&self, c: &oxc::ast::ast::JSXChild<'_>, name: &str) -> Apps {
        match c {
            oxc::ast::ast::JSXChild::Element(e) => self.count_jsx_element(e, name),
            oxc::ast::ast::JSXChild::Fragment(f) => self.count_jsx_fragment(f, name),
            oxc::ast::ast::JSXChild::ExpressionContainer(e) => e
                .expression
                .as_expression()
                .map(|x| self.count(x, name))
                .unwrap_or_default(),
            oxc::ast::ast::JSXChild::Spread(s) => self.count(&s.expression, name),
            oxc::ast::ast::JSXChild::Text(_) => Apps::default(),
        }
    }

    fn count_jsx_attr_value(&self, v: &oxc::ast::ast::JSXAttributeValue<'_>, name: &str) -> Apps {
        match v {
            oxc::ast::ast::JSXAttributeValue::ExpressionContainer(e) => e
                .expression
                .as_expression()
                .map(|x| self.count(x, name))
                .unwrap_or_default(),
            oxc::ast::ast::JSXAttributeValue::Element(e) => self.count_jsx_element(e, name),
            oxc::ast::ast::JSXAttributeValue::Fragment(f) => self.count_jsx_fragment(f, name),
            oxc::ast::ast::JSXAttributeValue::StringLiteral(_) => Apps::default(),
        }
    }

    fn count_call(&self, call: &CallExpression<'_>, name: &str) -> Apps {
        if let Some((recv, sig)) = self.instance_sig(call) {
            let mut apps = Apps::default();
            if let Some(object) = instance_member_object(call) {
                if ident_name(object).as_deref() != Some(name) {
                    apps = apps.merge(self.count(object, name));
                }
            }
            if recv == name {
                let mode = self.parameter_mode(sig.params.first().map(|(_, ty)| ty));
                apps = apps.merge(self.count_arg_ident(name, mode));
            }
            for (i, arg) in call.arguments.iter().enumerate() {
                match arg {
                    oxc::ast::ast::Argument::SpreadElement(s) => {
                        apps = apps.merge(self.count(&s.argument, name));
                    }
                    other => {
                        if let Some(expr) = other.as_expression() {
                            let mode = self.arg_mode(expr, Some(&sig), i + 1);
                            apps = apps.merge(self.count_arg(expr, name, mode));
                        }
                    }
                }
            }
            return apps;
        }
        let mut apps = self.count(&call.callee, name);
        let callee = callee_name(&call.callee);
        let sig = callee.as_ref().and_then(|n| self.callee_sig(n));
        for (i, arg) in call.arguments.iter().enumerate() {
            match arg {
                oxc::ast::ast::Argument::SpreadElement(s) => {
                    let spread = if sig.is_none() && !self.features.unknown_call_conservatism {
                        self.as_non_consuming(self.count(&s.argument, name))
                    } else {
                        self.count(&s.argument, name)
                    };
                    apps = apps.merge(spread);
                }
                other => {
                    if let Some(expr) = other.as_expression() {
                        let mode = self.arg_mode(expr, sig, i);
                        apps = apps.merge(self.count_arg(expr, name, mode));
                    }
                }
            }
        }
        apps
    }

    fn count_arg_ident(&self, name: &str, mode: ArgMode) -> Apps {
        if self.suppress_consume.as_deref() == Some(name) && mode == ArgMode::Consume {
            return Apps::default();
        }
        match mode {
            ArgMode::Consume => Apps {
                consumed: 1,
                ..Apps::default()
            },
            ArgMode::Read => Apps {
                read: 1,
                ..Apps::default()
            },
            ArgMode::Write => Apps {
                write: 1,
                ..Apps::default()
            },
            ArgMode::Path => Apps {
                path: 1,
                ..Apps::default()
            },
        }
    }

    fn as_non_consuming(&self, apps: Apps) -> Apps {
        Apps {
            consumed: 0,
            path: apps.path + apps.consumed,
            ..apps
        }
    }

    fn arg_mode(&self, expr: &Expression<'_>, sig: Option<&FnSig>, i: usize) -> ArgMode {
        let start = expr.span().start;
        if self.features.local_borrow_directives && self.features.borrow_model {
            for d in self.file.dirs_at(start) {
                match d {
                    OwnDirective::Shorthand(BorrowMode::Read) => return ArgMode::Read,
                    OwnDirective::Shorthand(BorrowMode::Write) => return ArgMode::Write,
                    _ => {}
                }
            }
        }
        if let Some(sig) = sig {
            if let Some((_, ty)) = sig.params.get(i) {
                return self.parameter_mode(Some(ty));
            }
            // Extra args on a known callee inherit last copy/ref mode, else Path
            // (varargs such as console.log must not consume).
            if let Some((_, ty)) = sig.params.last() {
                return match self.parameter_mode(Some(ty)) {
                    ArgMode::Read => ArgMode::Read,
                    ArgMode::Write => ArgMode::Write,
                    _ => ArgMode::Path,
                };
            }
            return ArgMode::Path;
        }
        if self.features.unknown_call_conservatism {
            ArgMode::Consume
        } else {
            ArgMode::Path
        }
    }

    fn parameter_mode(&self, ty: Option<&OwnType>) -> ArgMode {
        match ty {
            Some(OwnType::Unique(_) | OwnType::Affine(_)) if !self.features.move_tracking => {
                ArgMode::Path
            }
            Some(OwnType::Affine(_)) if !self.features.affine_kind => ArgMode::Path,
            Some(OwnType::RefRead(_) | OwnType::RefWrite(_)) if !self.features.borrow_model => {
                ArgMode::Path
            }
            _ => param_mode(ty),
        }
    }

    fn count_arg(&self, expr: &Expression<'_>, name: &str, mode: ArgMode) -> Apps {
        if ident_name(expr).as_deref() == Some(name) {
            return match mode {
                ArgMode::Consume => {
                    if self.suppress_consume.as_deref() == Some(name) {
                        Apps::default()
                    } else {
                        Apps {
                            consumed: 1,
                            ..Apps::default()
                        }
                    }
                }
                ArgMode::Read => Apps {
                    read: 1,
                    ..Apps::default()
                },
                ArgMode::Write => Apps {
                    write: 1,
                    ..Apps::default()
                },
                ArgMode::Path => Apps {
                    path: 1,
                    ..Apps::default()
                },
            };
        }
        self.count(expr, name)
    }

    fn apply_apps(&mut self, name: &str, apps: Apps, span: u32) {
        let apps = if self.features.non_consuming_paths {
            apps
        } else {
            Apps {
                consumed: apps.consumed + apps.path,
                path: 0,
                ..apps
            }
        };
        let Some(entry) = self.tbl.get(name).cloned() else {
            return;
        };
        if matches!(entry.kind, OwnKind::RefRead | OwnKind::RefWrite) {
            if apps.consumed > 0 {
                if let Some(owner) = entry.owner.clone() {
                    let mut consumed = Apps::default();
                    consumed.consumed = 1;
                    self.apply_apps(&owner, consumed, span);
                }
            }
            return;
        }
        let state = entry.state;
        let c = part(apps.consumed);
        let w = part(apps.write);
        let r = part(apps.read);
        let p = part(apps.path);
        match (state, c, w, r, p) {
            (VarState::Unconsumed, Part::Zero, Part::Zero, _, _) => {}
            (VarState::Unconsumed, Part::Zero, Part::One, Part::Zero, Part::Zero) => {}
            (VarState::Unconsumed, Part::Zero, Part::One, _, _) => {
                self.emit(
                    span,
                    RuleKind::MutBorrowConflict,
                    format!("`{name}` is borrowed mutably while also used another way"),
                );
            }
            (VarState::Unconsumed, Part::Zero, Part::More, _, _) => {
                self.emit(
                    span,
                    RuleKind::MutBorrowConflict,
                    format!("`{name}` is borrowed mutably more than once in the same expression"),
                );
            }
            (VarState::Unconsumed, Part::One, Part::Zero, Part::Zero, Part::Zero) => {
                self.consume_once(name, span);
            }
            (VarState::Unconsumed, Part::One, _, _, _) => {
                self.emit(
                    span,
                    RuleKind::DoubleMove,
                    format!(
                        "`{name}` is consumed and also borrowed or used in the same expression"
                    ),
                );
            }
            (VarState::Unconsumed, Part::More, _, _, _) => {
                self.emit(
                    span,
                    RuleKind::DoubleMove,
                    format!("`{name}` is consumed more than once in the same expression"),
                );
            }
            (VarState::BorrowedRead, Part::Zero, Part::Zero, Part::Zero, _) => {}
            (VarState::BorrowedRead, Part::Zero, Part::Zero, _, _) => {
                self.emit(
                    span,
                    RuleKind::MutBorrowConflict,
                    format!("cannot re-borrow `{name}` while it is borrowed (immutably)"),
                );
            }
            (VarState::BorrowedRead, _, _, _, _) => {
                self.emit(
                    span,
                    RuleKind::ConsumeWhileBorrowed,
                    format!("cannot consume `{name}` while it is borrowed (immutably)"),
                );
            }
            (VarState::BorrowedWrite, Part::Zero, Part::Zero, Part::Zero, Part::Zero) => {}
            (VarState::BorrowedWrite, _, _, _, _) => {
                let kind = if apps.consumed > 0 {
                    RuleKind::ConsumeWhileBorrowed
                } else {
                    RuleKind::MutBorrowConflict
                };
                self.emit(
                    span,
                    kind,
                    format!("cannot use `{name}` while it is borrowed (mutably)"),
                );
            }
            (VarState::Consumed, Part::Zero, Part::Zero, Part::Zero, Part::Zero) => {}
            (VarState::Consumed, _, _, _, _) => {
                let kind = if apps.read > 0 || apps.write > 0 {
                    RuleKind::BorrowAfterMove
                } else if apps.consumed > 0 {
                    RuleKind::UseAfterMove
                } else {
                    RuleKind::UseAfterMove
                };
                self.emit(span, kind, format!("`{name}` has already been consumed"));
            }
        }
    }

    fn consume_once(&mut self, name: &str, span: u32) {
        let Some(entry) = self.tbl.get(name) else {
            return;
        };
        if self.features.loop_depth && self.loop_depth != entry.loop_depth {
            self.emit(
                span,
                RuleKind::ConsumeInLoop,
                format!(
                    "variable `{name}` was defined outside a loop, but is consumed inside a loop"
                ),
            );
            return;
        }
        if let Some(e) = self.tbl.get_mut(name) {
            e.state = VarState::Consumed;
        }
    }

    fn check_class(&mut self, class: &oxc::ast::ast::Class<'_>) {
        if !self.enter_body(class.span.start) {
            return;
        }
        self.check_decorators(&class.decorators);
        if let Some(tp) = &class.type_parameters {
            self.check_ts_type_params(tp);
        }
        if let Some(h) = &class.heritage {
            self.check_expr(&h.expression, h.expression.span().start);
            self.check_discard(&h.expression);
            if let Some(ta) = &h.type_arguments {
                self.check_ts_type_args(ta);
            }
        }
        for impls in &class.implements {
            if let Some(ta) = &impls.type_arguments {
                self.check_ts_type_args(ta);
            }
        }
        for el in &class.body.body {
            match el {
                oxc::ast::ast::ClassElement::MethodDefinition(m) => {
                    let mut offs = vec![m.span.start, m.value.span.start];
                    if let oxc::ast::ast::PropertyKey::StaticIdentifier(id) = &m.key {
                        offs.push(id.span.start);
                    }
                    self.check_decorators(&m.decorators);
                    if let Some(k) = m.key.as_expression() {
                        self.check_expr(k, k.span().start);
                    }
                    self.check_discard_prop_key(&m.key);
                    self.check_function(&m.value, &offs);
                }
                oxc::ast::ast::ClassElement::PropertyDefinition(p) => {
                    self.check_decorators(&p.decorators);
                    if let Some(k) = p.key.as_expression() {
                        self.check_expr(k, k.span().start);
                    }
                    self.check_discard_prop_key(&p.key);
                    if let Some(t) = &p.type_annotation {
                        self.check_ts_ann(t);
                    }
                    if let Some(v) = &p.value {
                        self.check_methodish_expr(v, p.span.start);
                        self.check_expr(v, v.span().start);
                        self.check_discard(v);
                    }
                }
                oxc::ast::ast::ClassElement::StaticBlock(b) => {
                    self.push_scope();
                    for s in &b.body {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
                }
                oxc::ast::ast::ClassElement::TSIndexSignature(i) => {
                    self.check_ts_ann(&i.parameter.type_annotation);
                    self.check_ts_ann(&i.type_annotation);
                }
                oxc::ast::ast::ClassElement::AccessorProperty(p) => {
                    self.check_decorators(&p.decorators);
                    if let Some(k) = p.key.as_expression() {
                        self.check_expr(k, k.span().start);
                    }
                    self.check_discard_prop_key(&p.key);
                    if let Some(t) = &p.type_annotation {
                        self.check_ts_ann(t);
                    }
                    if let Some(v) = &p.value {
                        self.check_methodish_expr(v, p.span.start);
                        self.check_expr(v, v.span().start);
                        self.check_discard(v);
                    }
                }
            }
        }
    }

    fn check_object(&mut self, obj: &oxc::ast::ast::ObjectExpression<'_>) {
        if !self.enter_body(obj.span.start) {
            return;
        }
        for p in &obj.properties {
            match p {
                oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                    self.check_discard_prop_key(&p.key);
                    if let Some(k) = p.key.as_expression() {
                        self.check_contained_fn(k);
                    }
                    self.check_methodish_expr(&p.value, p.span.start);
                }
                oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                    self.check_contained_fn(&s.argument);
                }
            }
        }
    }

    fn check_methodish_expr(&mut self, expr: &Expression<'_>, extra: u32) {
        match peel(expr) {
            Expression::FunctionExpression(f) => {
                self.check_function(f, &[extra, f.span.start]);
            }
            Expression::ArrowFunctionExpression(a) => {
                let offs = [extra, a.span.start];
                if let Some(sig) = self.file.type_sig_at(&offs) {
                    self.check_arrow(a, &sig);
                } else {
                    self.check_arrow_unannotated(a);
                }
            }
            Expression::ObjectExpression(o) => self.check_object(o),
            Expression::ClassExpression(c) => self.check_class(c),
            Expression::AssignmentExpression(a) => self.check_methodish_expr(&a.right, extra),
            Expression::SequenceExpression(s) => {
                if let Some(e) = s.expressions.last() {
                    self.check_methodish_expr(e, extra);
                }
            }
            Expression::LogicalExpression(b) => {
                self.check_methodish_expr(&b.left, extra);
                self.check_methodish_expr(&b.right, extra);
            }
            Expression::ConditionalExpression(c) => {
                self.check_methodish_expr(&c.consequent, extra);
                self.check_methodish_expr(&c.alternate, extra);
            }
            _ => {}
        }
    }

    fn check_decorators(&mut self, decs: &[oxc::ast::ast::Decorator<'_>]) {
        for d in decs {
            self.check_expr(&d.expression, d.expression.span().start);
            self.check_discard(&d.expression);
        }
    }

    fn check_formal_params(
        &mut self,
        params: &oxc::ast::ast::FormalParameters<'_>,
        sig: Option<&FnSig>,
    ) {
        for (i, p) in params.items.iter().enumerate() {
            self.check_decorators(&p.decorators);
            if let Some(t) = &p.type_annotation {
                self.check_ts_ann(t);
            }
            self.check_discard_binding(&p.pattern);
            if let Some(init) = &p.initializer {
                self.check_expr(init, init.span().start);
                if sig.is_some() {
                    self.check_unique_arg(init, sig, i);
                } else {
                    self.check_discard(init);
                }
            }
        }
        if let Some(r) = &params.rest {
            self.check_decorators(&r.decorators);
            if let Some(t) = &r.type_annotation {
                self.check_ts_ann(t);
            }
            self.check_discard_binding(&r.rest.argument);
        }
    }

    fn check_ts_ann(&mut self, ann: &oxc::ast::ast::TSTypeAnnotation<'_>) {
        self.check_ts_type(&ann.type_annotation);
    }

    fn check_ts_type(&mut self, ty: &oxc::ast::ast::TSType<'_>) {
        match ty {
            oxc::ast::ast::TSType::TSTypeLiteral(l) => {
                for m in &l.members {
                    self.check_ts_signature(m);
                }
            }
            oxc::ast::ast::TSType::TSArrayType(a) => self.check_ts_type(&a.element_type),
            oxc::ast::ast::TSType::TSUnionType(u) => {
                for t in &u.types {
                    self.check_ts_type(t);
                }
            }
            oxc::ast::ast::TSType::TSIntersectionType(i) => {
                for t in &i.types {
                    self.check_ts_type(t);
                }
            }
            oxc::ast::ast::TSType::TSParenthesizedType(p) => self.check_ts_type(&p.type_annotation),
            oxc::ast::ast::TSType::TSTypeOperatorType(o) => self.check_ts_type(&o.type_annotation),
            oxc::ast::ast::TSType::TSIndexedAccessType(i) => {
                self.check_ts_type(&i.object_type);
                self.check_ts_type(&i.index_type);
            }
            oxc::ast::ast::TSType::TSConditionalType(c) => {
                self.check_ts_type(&c.check_type);
                self.check_ts_type(&c.extends_type);
                self.check_ts_type(&c.true_type);
                self.check_ts_type(&c.false_type);
            }
            oxc::ast::ast::TSType::TSTupleType(t) => {
                for el in &t.element_types {
                    if let Some(inner) = el.as_ts_type() {
                        self.check_ts_type(inner);
                    } else {
                        match el {
                            oxc::ast::ast::TSTupleElement::TSOptionalType(o) => {
                                self.check_ts_type(&o.type_annotation);
                            }
                            oxc::ast::ast::TSTupleElement::TSRestType(r) => {
                                self.check_ts_type(&r.type_annotation);
                            }
                            _ => {}
                        }
                    }
                }
            }
            oxc::ast::ast::TSType::TSFunctionType(f) => {
                if let Some(tp) = &f.type_parameters {
                    self.check_ts_type_params(tp);
                }
                if let Some(this) = &f.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.check_ts_ann(t);
                    }
                }
                self.check_ts_params(&f.params);
                self.check_ts_ann(&f.return_type);
            }
            oxc::ast::ast::TSType::TSConstructorType(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.check_ts_type_params(tp);
                }
                self.check_ts_params(&c.params);
                self.check_ts_ann(&c.return_type);
            }
            oxc::ast::ast::TSType::TSInferType(i) => {
                if let Some(c) = &i.type_parameter.constraint {
                    self.check_ts_type(c);
                }
                if let Some(d) = &i.type_parameter.default {
                    self.check_ts_type(d);
                }
            }
            oxc::ast::ast::TSType::JSDocNullableType(t) => self.check_ts_type(&t.type_annotation),
            oxc::ast::ast::TSType::JSDocNonNullableType(t) => {
                self.check_ts_type(&t.type_annotation)
            }
            oxc::ast::ast::TSType::TSTemplateLiteralType(t) => {
                for inner in &t.types {
                    self.check_ts_type(inner);
                }
            }
            oxc::ast::ast::TSType::TSTypeReference(r) => {
                if let Some(ta) = &r.type_arguments {
                    self.check_ts_type_args(ta);
                }
            }
            oxc::ast::ast::TSType::TSMappedType(m) => {
                self.check_ts_type(&m.constraint);
                if let Some(n) = &m.name_type {
                    self.check_ts_type(n);
                }
                if let Some(a) = &m.type_annotation {
                    self.check_ts_type(a);
                }
            }
            oxc::ast::ast::TSType::TSNamedTupleMember(m) => {
                if let Some(inner) = m.element_type.as_ts_type() {
                    self.check_ts_type(inner);
                }
            }
            oxc::ast::ast::TSType::TSTypePredicate(p) => {
                if let Some(t) = &p.type_annotation {
                    self.check_ts_ann(t);
                }
            }
            oxc::ast::ast::TSType::TSImportType(i) => self.check_ts_import_type(i),
            oxc::ast::ast::TSType::TSTypeQuery(q) => {
                if let oxc::ast::ast::TSTypeQueryExprName::TSImportType(i) = &q.expr_name {
                    self.check_ts_import_type(i);
                }
                if let Some(ta) = &q.type_arguments {
                    self.check_ts_type_args(ta);
                }
            }
            oxc::ast::ast::TSType::TSLiteralType(l) => match &l.literal {
                oxc::ast::ast::TSLiteral::TemplateLiteral(t) => {
                    for e in &t.expressions {
                        self.check_discard(e);
                    }
                }
                oxc::ast::ast::TSLiteral::UnaryExpression(u) => {
                    self.check_discard(&u.argument);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn check_ts_import_type(&mut self, i: &oxc::ast::ast::TSImportType<'_>) {
        if let Some(o) = &i.options {
            self.check_object(o);
            for p in &o.properties {
                match p {
                    oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                        self.check_discard_prop_key(&p.key);
                        self.check_discard(&p.value);
                    }
                    oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                        self.check_discard(&s.argument);
                    }
                }
            }
        }
        if let Some(ta) = &i.type_arguments {
            self.check_ts_type_args(ta);
        }
    }

    fn check_ts_type_args(&mut self, ta: &oxc::ast::ast::TSTypeParameterInstantiation<'_>) {
        for t in &ta.params {
            self.check_ts_type(t);
        }
    }

    fn check_ts_type_params(&mut self, tp: &oxc::ast::ast::TSTypeParameterDeclaration<'_>) {
        for p in &tp.params {
            if let Some(c) = &p.constraint {
                self.check_ts_type(c);
            }
            if let Some(d) = &p.default {
                self.check_ts_type(d);
            }
        }
    }

    fn check_ts_interface(&mut self, i: &oxc::ast::ast::TSInterfaceDeclaration<'_>) {
        if let Some(tp) = &i.type_parameters {
            self.check_ts_type_params(tp);
        }
        for e in &i.extends {
            if let Some(ta) = &e.type_arguments {
                self.check_ts_type_args(ta);
            }
        }
        for s in &i.body.body {
            self.check_ts_signature(s);
        }
    }

    fn check_ts_params(&mut self, params: &oxc::ast::ast::FormalParameters<'_>) {
        for p in &params.items {
            if let Some(t) = &p.type_annotation {
                self.check_ts_ann(t);
            }
        }
        if let Some(r) = &params.rest {
            if let Some(t) = &r.type_annotation {
                self.check_ts_ann(t);
            }
        }
    }

    fn check_ts_signature(&mut self, s: &oxc::ast::ast::TSSignature<'_>) {
        match s {
            oxc::ast::ast::TSSignature::TSPropertySignature(p) => {
                self.check_discard_prop_key(&p.key);
                if let Some(t) = &p.type_annotation {
                    self.check_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSMethodSignature(m) => {
                self.check_discard_prop_key(&m.key);
                if let Some(tp) = &m.type_parameters {
                    self.check_ts_type_params(tp);
                }
                if let Some(this) = &m.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.check_ts_ann(t);
                    }
                }
                self.check_ts_params(&m.params);
                if let Some(t) = &m.return_type {
                    self.check_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSIndexSignature(i) => {
                self.check_ts_ann(&i.parameter.type_annotation);
                self.check_ts_ann(&i.type_annotation);
            }
            oxc::ast::ast::TSSignature::TSCallSignatureDeclaration(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.check_ts_type_params(tp);
                }
                if let Some(this) = &c.this_param {
                    if let Some(t) = &this.type_annotation {
                        self.check_ts_ann(t);
                    }
                }
                self.check_ts_params(&c.params);
                if let Some(t) = &c.return_type {
                    self.check_ts_ann(t);
                }
            }
            oxc::ast::ast::TSSignature::TSConstructSignatureDeclaration(c) => {
                if let Some(tp) = &c.type_parameters {
                    self.check_ts_type_params(tp);
                }
                self.check_ts_params(&c.params);
                if let Some(t) = &c.return_type {
                    self.check_ts_ann(t);
                }
            }
        }
    }

    fn check_ts_enum(&mut self, e: &oxc::ast::ast::TSEnumDeclaration<'_>) {
        for m in &e.body.members {
            if let oxc::ast::ast::TSEnumMemberName::ComputedTemplateString(t) = &m.id {
                for expr in &t.expressions {
                    self.check_expr(expr, expr.span().start);
                    self.check_discard(expr);
                }
            }
            if let Some(init) = &m.initializer {
                self.check_expr(init, init.span().start);
                self.check_discard(init);
            }
        }
    }

    fn check_ts_namespace(&mut self, n: &oxc::ast::ast::TSNamespaceDeclaration<'_>) {
        let namespace = self.namespace_scopes.last().map_or_else(
            || n.id.name.to_string(),
            |prefix| format!("{prefix}.{}", n.id.name),
        );
        self.namespace_scopes.push(namespace);
        match &n.body {
            oxc::ast::ast::TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
                self.check_ts_namespace(inner);
            }
            oxc::ast::ast::TSNamespaceDeclarationBody::TSModuleBlock(b) => {
                self.check_ts_module_block(b);
            }
        }
        self.namespace_scopes.pop();
    }

    fn check_ts_module_block(&mut self, b: &oxc::ast::ast::TSModuleBlock<'_>) {
        self.push_scope();
        for s in &b.body {
            self.check_stmt(s);
        }
        self.pop_scope();
    }

    fn check_jsx_element(&mut self, el: &oxc::ast::ast::JSXElement<'_>) {
        for attr in &el.opening_element.attributes {
            match attr {
                oxc::ast::ast::JSXAttributeItem::Attribute(a) => {
                    if let Some(v) = &a.value {
                        self.check_jsx_attr_value(v);
                    }
                }
                oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                    self.check_expr(&s.argument, s.argument.span().start);
                    self.check_discard(&s.argument);
                }
            }
        }
        for c in &el.children {
            self.check_jsx_child(c);
        }
    }

    fn check_jsx_fragment(&mut self, f: &oxc::ast::ast::JSXFragment<'_>) {
        for c in &f.children {
            self.check_jsx_child(c);
        }
    }

    fn check_jsx_child(&mut self, c: &oxc::ast::ast::JSXChild<'_>) {
        match c {
            oxc::ast::ast::JSXChild::Element(e) => self.check_jsx_element(e),
            oxc::ast::ast::JSXChild::Fragment(f) => self.check_jsx_fragment(f),
            oxc::ast::ast::JSXChild::ExpressionContainer(e) => {
                self.check_jsx_expression(&e.expression);
            }
            oxc::ast::ast::JSXChild::Spread(s) => {
                self.check_expr(&s.expression, s.expression.span().start);
                self.check_discard(&s.expression);
            }
            oxc::ast::ast::JSXChild::Text(_) => {}
        }
    }

    fn check_jsx_attr_value(&mut self, v: &oxc::ast::ast::JSXAttributeValue<'_>) {
        match v {
            oxc::ast::ast::JSXAttributeValue::ExpressionContainer(e) => {
                self.check_jsx_expression(&e.expression);
            }
            oxc::ast::ast::JSXAttributeValue::Element(e) => self.check_jsx_element(e),
            oxc::ast::ast::JSXAttributeValue::Fragment(f) => self.check_jsx_fragment(f),
            oxc::ast::ast::JSXAttributeValue::StringLiteral(_) => {}
        }
    }

    fn check_jsx_expression(&mut self, e: &oxc::ast::ast::JSXExpression<'_>) {
        if let Some(expr) = e.as_expression() {
            self.check_discard(expr);
        }
    }

    fn check_arrow_unannotated(&mut self, arrow: &ArrowFunctionExpression<'_>) {
        if !self.enter_body(arrow.span.start) {
            return;
        }
        self.callee_scopes.push(arrow.span.start);
        if let Some(tp) = &arrow.type_parameters {
            self.check_ts_type_params(tp);
        }
        if let Some(rt) = &arrow.return_type {
            self.check_ts_ann(rt);
        }
        self.report_captures_arrow(arrow);
        let saved_tbl = self.tbl.clone();
        let saved_scopes = self.scopes.clone();
        let saved_depth = self.loop_depth;
        let saved_ret = self.fn_ret.clone();
        let saved_finally = self.try_finally_depth;
        let saved_pending = std::mem::take(&mut self.pending_finally);
        self.tbl.clear();
        self.scopes.clear();
        self.loop_depth = 0;
        self.fn_ret = None;
        self.try_finally_depth = 0;
        self.pending_finally.clear();
        self.push_scope();
        self.check_formal_params(&arrow.params, None);
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => {
                for s in &body.statements {
                    self.check_stmt(s);
                }
            }
            other => {
                if let Some(expr) = other.as_expression() {
                    self.check_expr(expr, expr.span().start);
                    self.check_discard(expr);
                }
            }
        }
        self.pop_scope();
        self.tbl = saved_tbl;
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
        self.fn_ret = saved_ret;
        self.try_finally_depth = saved_finally;
        self.pending_finally = saved_pending;
        self.callee_scopes.pop();
    }

    fn report_captures_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>) {
        let owned = capture_candidates_for_params(
            self.tbl.keys().cloned().collect(),
            &arrow.params,
            None,
        );
        if owned.is_empty() {
            return;
        }
        for p in &arrow.params.items {
            if let Some(init) = &p.initializer {
                self.report_captures_expr(init, &owned);
            }
        }
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => self.report_captures_in_body(body, &owned),
            other => {
                if let Some(e) = other.as_expression() {
                    self.report_captures_expr(e, &owned);
                }
            }
        }
    }

    fn check_param_default_captures(&mut self, func: &Function<'_>) {
        let owned = capture_candidates_for_function(self.tbl.keys().cloned().collect(), func);
        if owned.is_empty() {
            return;
        }
        for p in &func.params.items {
            if let Some(init) = &p.initializer {
                self.report_captures_expr(init, &owned);
            }
        }
    }

    fn check_tagged_template_parts(&mut self, t: &oxc::ast::ast::TaggedTemplateExpression<'_>) {
        if let Some(ta) = &t.type_arguments {
            self.check_ts_type_args(ta);
        }
        self.check_discard(&t.tag);
        let sig = callee_name(&t.tag).and_then(|n| self.callee_sig(&n).cloned());
        for (i, e) in t.quasi.expressions.iter().enumerate() {
            self.check_unique_arg(e, sig.as_ref(), i + 1);
        }
    }

    fn check_unique_rvalues(&mut self, expr: &Expression<'_>) {
        if let Some(call) = as_call(expr) {
            if let Some(ta) = &call.type_arguments {
                self.check_ts_type_args(ta);
            }
            if let Some((recv, sig)) = self.instance_sig(call) {
                let _ = recv;
                for (i, arg) in call.arguments.iter().enumerate() {
                    match arg {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_unique_arg(e, Some(&sig), i + 1);
                            }
                        }
                    }
                }
            } else {
                let callee = callee_name(&call.callee);
                let sig = callee.as_ref().and_then(|n| self.callee_sig(n).cloned());
                for (i, arg) in call.arguments.iter().enumerate() {
                    match arg {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_unique_arg(e, sig.as_ref(), i);
                            }
                        }
                    }
                }
            }
            return;
        }
        match peel(expr) {
            Expression::NewExpression(n) => {
                if let Some(ta) = &n.type_arguments {
                    self.check_ts_type_args(ta);
                }
                let callee = callee_name(&n.callee);
                let sig = callee.as_ref().and_then(|n| self.callee_sig(n).cloned());
                for (i, arg) in n.arguments.iter().enumerate() {
                    match arg {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_unique_arg(e, sig.as_ref(), i);
                            }
                        }
                    }
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                self.check_tagged_template_parts(t);
            }
            Expression::V8IntrinsicExpression(v) => {
                for (i, a) in v.arguments.iter().enumerate() {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_discard(&s.argument);
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_unique_arg(e, None, i);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn check_unique_arg(&mut self, expr: &Expression<'_>, sig: Option<&FnSig>, i: usize) {
        self.walk_type_wrappers(expr);
        match peel(expr) {
            Expression::LogicalExpression(b) => {
                match b.operator {
                    LogicalOperator::And => {
                        self.check_discard(&b.left);
                        self.check_unique_arg(&b.right, sig, i);
                    }
                    LogicalOperator::Or | LogicalOperator::Coalesce => {
                        if matches!(self.call_return_type(&b.left), Some(OwnType::Unique(_))) {
                            self.check_unique_arg(&b.left, sig, i);
                            self.check_discard(&b.right);
                        } else {
                            self.check_discard(&b.left);
                            self.check_unique_arg(&b.right, sig, i);
                        }
                    }
                }
                return;
            }
            Expression::ConditionalExpression(c) => {
                self.check_discard(&c.test);
                self.check_unique_arg(&c.consequent, sig, i);
                self.check_unique_arg(&c.alternate, sig, i);
                return;
            }
            Expression::SequenceExpression(s) => {
                let n = s.expressions.len();
                for (j, e) in s.expressions.iter().enumerate() {
                    if j + 1 == n {
                        self.check_unique_arg(e, sig, i);
                    } else {
                        self.check_discard(e);
                    }
                }
                return;
            }
            Expression::AssignmentExpression(a) => {
                self.check_discard_assignment_target(&a.left);
                self.check_unique_arg(&a.right, sig, i);
                return;
            }
            Expression::UnaryExpression(u) if matches!(u.operator, UnaryOperator::Void) => {
                self.check_discard(&u.argument);
                return;
            }
            _ => {}
        }
        let mode = self.arg_mode(expr, sig, i);
        if mode != ArgMode::Consume {
            self.check_discard(expr);
            return;
        }
        if !matches!(self.call_return_type(expr), Some(OwnType::Unique(_))) {
            self.check_discard(expr);
        }
        match peel(expr) {
            Expression::CallExpression(_)
            | Expression::NewExpression(_)
            | Expression::ChainExpression(_)
            | Expression::TaggedTemplateExpression(_)
            | Expression::V8IntrinsicExpression(_) => {
                self.check_unique_rvalues(expr);
            }
            _ => {}
        }
    }

    fn visit_exclusive_nested(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::CallExpression(c) => {
                self.visit_exclusive_nested(&c.callee);
                for a in &c.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.visit_exclusive_maybe(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.visit_exclusive_maybe(e);
                            }
                        }
                    }
                }
            }
            Expression::NewExpression(n) => {
                self.visit_exclusive_nested(&n.callee);
                for a in &n.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.visit_exclusive_maybe(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.visit_exclusive_maybe(e);
                            }
                        }
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.visit_exclusive_maybe(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.visit_exclusive_maybe(e);
                            }
                        }
                    }
                }
            }
            Expression::ObjectExpression(o) => {
                for p in &o.properties {
                    match p {
                        oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) => {
                            if let Some(k) = p.key.as_expression() {
                                self.visit_exclusive_maybe(k);
                            }
                            self.visit_exclusive_maybe(&p.value);
                        }
                        oxc::ast::ast::ObjectPropertyKind::SpreadProperty(s) => {
                            self.visit_exclusive_maybe(&s.argument)
                        }
                    }
                }
            }
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.visit_exclusive_maybe(e);
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                self.visit_exclusive_maybe(&t.tag);
                for e in &t.quasi.expressions {
                    self.visit_exclusive_maybe(e);
                }
            }
            Expression::YieldExpression(y) => {
                if let Some(a) = &y.argument {
                    self.visit_exclusive_maybe(a);
                }
            }
            Expression::PrivateInExpression(p) => self.visit_exclusive_maybe(&p.right),
            Expression::JSXElement(el) => self.visit_exclusive_jsx_element(el),
            Expression::JSXFragment(f) => self.visit_exclusive_jsx_fragment(f),
            Expression::ParenthesizedExpression(p) => self.visit_exclusive_maybe(&p.expression),
            Expression::AwaitExpression(a) => self.visit_exclusive_maybe(&a.argument),
            Expression::UnaryExpression(u) => self.visit_exclusive_maybe(&u.argument),
            Expression::BinaryExpression(b) => {
                self.visit_exclusive_maybe(&b.left);
                self.visit_exclusive_maybe(&b.right);
            }
            Expression::AssignmentExpression(a) => {
                self.visit_exclusive_assignment_target(&a.left);
                self.visit_exclusive_maybe(&a.right);
            }
            Expression::UpdateExpression(u) => match &u.argument {
                oxc::ast::ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                    self.visit_exclusive_maybe(&m.object);
                    self.visit_exclusive_maybe(&m.expression);
                }
                oxc::ast::ast::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                    self.visit_exclusive_maybe(&m.object)
                }
                oxc::ast::ast::SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                    self.visit_exclusive_maybe(&m.object)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSAsExpression(e) => {
                    self.visit_exclusive_maybe(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => {
                    self.visit_exclusive_maybe(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSNonNullExpression(e) => {
                    self.visit_exclusive_maybe(&e.expression)
                }
                oxc::ast::ast::SimpleAssignmentTarget::TSTypeAssertion(e) => {
                    self.visit_exclusive_maybe(&e.expression)
                }
                _ => {}
            },
            Expression::ImportExpression(i) => {
                self.visit_exclusive_maybe(&i.source);
                if let Some(o) = &i.options {
                    self.visit_exclusive_maybe(o);
                }
            }
            Expression::TSAsExpression(e) => self.visit_exclusive_maybe(&e.expression),
            Expression::TSSatisfiesExpression(e) => self.visit_exclusive_maybe(&e.expression),
            Expression::TSNonNullExpression(e) => self.visit_exclusive_maybe(&e.expression),
            Expression::TSTypeAssertion(e) => self.visit_exclusive_maybe(&e.expression),
            Expression::TSInstantiationExpression(e) => self.visit_exclusive_maybe(&e.expression),
            Expression::V8IntrinsicExpression(v) => {
                for a in &v.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.visit_exclusive_maybe(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.visit_exclusive_maybe(e);
                            }
                        }
                    }
                }
            }
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.visit_exclusive_maybe(e);
                }
            }
            Expression::ComputedMemberExpression(m) => {
                self.visit_exclusive_maybe(&m.object);
                self.visit_exclusive_maybe(&m.expression);
            }
            Expression::StaticMemberExpression(m) => self.visit_exclusive_maybe(&m.object),
            Expression::PrivateFieldExpression(m) => self.visit_exclusive_maybe(&m.object),
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => {
                    self.visit_exclusive_nested(&call.callee);
                    for a in &call.arguments {
                        match a {
                            oxc::ast::ast::Argument::SpreadElement(s) => {
                                self.visit_exclusive_maybe(&s.argument)
                            }
                            other => {
                                if let Some(e) = other.as_expression() {
                                    self.visit_exclusive_maybe(e);
                                }
                            }
                        }
                    }
                }
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.visit_exclusive_maybe(&m.object);
                    self.visit_exclusive_maybe(&m.expression);
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                    self.visit_exclusive_maybe(&m.object)
                }
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => {
                    self.visit_exclusive_maybe(&m.object)
                }
                oxc::ast::ast::ChainElement::TSNonNullExpression(n) => {
                    self.visit_exclusive_maybe(&n.expression)
                }
            },
            _ => {}
        }
    }

    fn visit_exclusive_jsx_element(&mut self, el: &oxc::ast::ast::JSXElement<'_>) {
        for attr in &el.opening_element.attributes {
            match attr {
                oxc::ast::ast::JSXAttributeItem::Attribute(at) => {
                    if let Some(oxc::ast::ast::JSXAttributeValue::ExpressionContainer(e)) =
                        &at.value
                    {
                        if let Some(x) = e.expression.as_expression() {
                            self.visit_exclusive_maybe(x);
                        }
                    }
                }
                oxc::ast::ast::JSXAttributeItem::SpreadAttribute(s) => {
                    self.visit_exclusive_maybe(&s.argument)
                }
            }
        }
        for c in &el.children {
            self.visit_exclusive_jsx_child(c);
        }
    }

    fn visit_exclusive_jsx_fragment(&mut self, f: &oxc::ast::ast::JSXFragment<'_>) {
        for c in &f.children {
            self.visit_exclusive_jsx_child(c);
        }
    }

    fn visit_exclusive_jsx_child(&mut self, c: &oxc::ast::ast::JSXChild<'_>) {
        match c {
            oxc::ast::ast::JSXChild::Element(e) => self.visit_exclusive_jsx_element(e),
            oxc::ast::ast::JSXChild::Fragment(f) => self.visit_exclusive_jsx_fragment(f),
            oxc::ast::ast::JSXChild::ExpressionContainer(e) => {
                if let Some(x) = e.expression.as_expression() {
                    self.visit_exclusive_maybe(x);
                }
            }
            oxc::ast::ast::JSXChild::Spread(s) => self.visit_exclusive_maybe(&s.expression),
            oxc::ast::ast::JSXChild::Text(_) => {}
        }
    }

    fn visit_exclusive_maybe(&mut self, expr: &Expression<'_>) {
        let expr = peel(expr);
        match expr {
            Expression::LogicalExpression(_) | Expression::ConditionalExpression(_) => {
                self.check_expr(expr, expr.span().start);
            }
            _ => self.visit_exclusive_nested(expr),
        }
    }

    fn check_nested_fn_values(&mut self, expr: &Expression<'_>) {
        match peel(expr) {
            Expression::FunctionExpression(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::ObjectExpression(_)
            | Expression::ClassExpression(_) => {
                self.check_contained_fn(expr);
            }
            Expression::CallExpression(c) => {
                self.check_contained_fn(&c.callee);
                for a in &c.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_contained_fn(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_contained_fn(e);
                            }
                        }
                    }
                }
            }
            Expression::NewExpression(n) => {
                self.check_contained_fn(&n.callee);
                for a in &n.arguments {
                    match a {
                        oxc::ast::ast::Argument::SpreadElement(s) => {
                            self.check_contained_fn(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_contained_fn(e);
                            }
                        }
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for el in &arr.elements {
                    match el {
                        oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                            self.check_contained_fn(&s.argument)
                        }
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_contained_fn(e);
                            }
                        }
                    }
                }
            }
            Expression::TemplateLiteral(t) => {
                for e in &t.expressions {
                    self.check_contained_fn(e);
                }
            }
            Expression::TaggedTemplateExpression(t) => {
                self.check_contained_fn(&t.tag);
                for e in &t.quasi.expressions {
                    self.check_contained_fn(e);
                }
            }
            Expression::YieldExpression(y) => {
                if let Some(a) = &y.argument {
                    self.check_contained_fn(a);
                }
            }
            Expression::ComputedMemberExpression(m) => {
                self.check_contained_fn(&m.object);
                self.check_contained_fn(&m.expression);
            }
            Expression::StaticMemberExpression(m) => self.check_contained_fn(&m.object),
            Expression::SequenceExpression(s) => {
                for e in &s.expressions {
                    self.check_contained_fn(e);
                }
            }
            Expression::ChainExpression(c) => {
                if let oxc::ast::ast::ChainElement::CallExpression(call) = &c.expression {
                    self.check_contained_fn(&call.callee);
                    for a in &call.arguments {
                        if let Some(e) = a.as_expression() {
                            self.check_contained_fn(e);
                        }
                    }
                }
            }
            Expression::LogicalExpression(b) => {
                self.check_contained_fn(&b.left);
                self.check_contained_fn(&b.right);
            }
            Expression::BinaryExpression(b) => {
                self.check_contained_fn(&b.left);
                self.check_contained_fn(&b.right);
            }
            Expression::UnaryExpression(u) => self.check_contained_fn(&u.argument),
            Expression::ConditionalExpression(c) => {
                self.check_contained_fn(&c.test);
                self.check_contained_fn(&c.consequent);
                self.check_contained_fn(&c.alternate);
            }
            Expression::ParenthesizedExpression(p) => self.check_contained_fn(&p.expression),
            Expression::AwaitExpression(a) => self.check_contained_fn(&a.argument),
            Expression::AssignmentExpression(a) => self.check_contained_fn(&a.right),
            Expression::PrivateInExpression(p) => self.check_contained_fn(&p.right),
            _ => {}
        }
    }

    fn check_contained_fn(&mut self, expr: &Expression<'_>) {
        match peel(expr) {
            Expression::FunctionExpression(f) => {
                self.check_function(f, &[f.span.start]);
            }
            Expression::ArrowFunctionExpression(a) => {
                if let Some(sig) = self.file.type_sig_at(&[a.span.start]) {
                    self.check_arrow(a, &sig);
                } else {
                    self.check_arrow_unannotated(a);
                }
            }
            Expression::ObjectExpression(o) => self.check_object(o),
            Expression::ClassExpression(c) => self.check_class(c),
            _ => self.check_nested_fn_values(expr),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgMode {
    Consume,
    Read,
    Write,
    Path,
}

fn capture_candidates_for_function(owned: Vec<String>, function: &Function<'_>) -> Vec<String> {
    capture_candidates_for_params(
        owned,
        &function.params,
        function.id.as_ref().map(|id| id.name.as_str()),
    )
}

fn capture_candidates_for_params(
    mut owned: Vec<String>,
    params: &oxc::ast::ast::FormalParameters<'_>,
    local_name: Option<&str>,
) -> Vec<String> {
    let mut bound = HashSet::new();
    if let Some(name) = local_name {
        bound.insert(name.to_string());
    }
    for param in &params.items {
        collect_binding_names(&param.pattern, &mut bound);
    }
    if let Some(rest) = &params.rest {
        collect_binding_names(&rest.rest.argument, &mut bound);
    }
    owned.retain(|name| !bound.contains(name));
    owned
}

fn capture_candidates_after_local_declarations(
    owned: &[String],
    statements: &[Statement<'_>],
) -> Vec<String> {
    let mut bound = HashSet::new();
    for statement in statements {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    collect_binding_names(&declarator.id, &mut bound);
                }
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    bound.insert(id.name.as_str().to_string());
                }
            }
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    bound.insert(id.name.as_str().to_string());
                }
            }
            _ => {}
        }
    }
    owned
        .iter()
        .filter(|name| !bound.contains(name.as_str()))
        .cloned()
        .collect()
}

fn capture_candidates_after_hoisted_vars(
    owned: &[String],
    statements: &[Statement<'_>],
) -> Vec<String> {
    let mut bound = HashSet::new();
    collect_hoisted_var_names(statements, &mut bound);
    owned
        .iter()
        .filter(|name| !bound.contains(name.as_str()))
        .cloned()
        .collect()
}

fn collect_hoisted_var_names(statements: &[Statement<'_>], out: &mut HashSet<String>) {
    for statement in statements {
        match statement {
            Statement::VariableDeclaration(declaration)
                if declaration.kind == oxc::ast::ast::VariableDeclarationKind::Var =>
            {
                for declarator in &declaration.declarations {
                    collect_binding_names(&declarator.id, out);
                }
            }
            Statement::BlockStatement(block) => collect_hoisted_var_names(&block.body, out),
            Statement::IfStatement(statement) => {
                collect_hoisted_var_names(std::slice::from_ref(&statement.consequent), out);
                if let Some(alternate) = &statement.alternate {
                    collect_hoisted_var_names(std::slice::from_ref(alternate), out);
                }
            }
            Statement::WhileStatement(statement) => {
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::DoWhileStatement(statement) => {
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::ForStatement(statement) => {
                if let Some(ForStatementInit::VariableDeclaration(declaration)) = &statement.init {
                    if declaration.kind == oxc::ast::ast::VariableDeclarationKind::Var {
                        for declarator in &declaration.declarations {
                            collect_binding_names(&declarator.id, out);
                        }
                    }
                }
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::ForInStatement(statement) => {
                collect_hoisted_for_left(&statement.left, out);
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::ForOfStatement(statement) => {
                collect_hoisted_for_left(&statement.left, out);
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::SwitchStatement(statement) => {
                for case in &statement.cases {
                    collect_hoisted_var_names(&case.consequent, out);
                }
            }
            Statement::TryStatement(statement) => {
                collect_hoisted_var_names(&statement.block.body, out);
                if let Some(handler) = &statement.handler {
                    collect_hoisted_var_names(&handler.body.body, out);
                }
                if let Some(finalizer) = &statement.finalizer {
                    collect_hoisted_var_names(&finalizer.body, out);
                }
            }
            Statement::LabeledStatement(statement) => {
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            Statement::WithStatement(statement) => {
                collect_hoisted_var_names(std::slice::from_ref(&statement.body), out);
            }
            // Nested functions and classes own their `var` declarations.
            Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => {}
            _ => {}
        }
    }
}

fn collect_hoisted_for_left(
    left: &oxc::ast::ast::ForStatementLeft<'_>,
    out: &mut HashSet<String>,
) {
    if let oxc::ast::ast::ForStatementLeft::VariableDeclaration(declaration) = left {
        if declaration.kind == oxc::ast::ast::VariableDeclarationKind::Var {
            for declarator in &declaration.declarations {
                collect_binding_names(&declarator.id, out);
            }
        }
    }
}

fn collect_binding_names(pattern: &BindingPattern<'_>, out: &mut HashSet<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            out.insert(identifier.name.as_str().to_string());
        }
        BindingPattern::AssignmentPattern(assignment) => {
            collect_binding_names(&assignment.left, out);
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                collect_binding_names(&property.value, out);
            }
            if let Some(rest) = &object.rest {
                collect_binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                collect_binding_names(element, out);
            }
            if let Some(rest) = &array.rest {
                collect_binding_names(&rest.argument, out);
            }
        }
    }
}

fn param_mode(ty: Option<&OwnType>) -> ArgMode {
    match ty {
        Some(OwnType::RefRead(_)) => ArgMode::Read,
        Some(OwnType::RefWrite(_)) => ArgMode::Write,
        Some(OwnType::Copy(_)) | Some(OwnType::Void) => ArgMode::Path,
        Some(_) => ArgMode::Consume,
        None => ArgMode::Consume,
    }
}

fn ident_of_pattern(pat: &BindingPattern<'_>) -> Option<String> {
    pat.get_identifier_name().map(|n| n.as_str().to_string())
}

fn ident_name(expr: &Expression<'_>) -> Option<String> {
    match peel(expr) {
        Expression::Identifier(id) => Some(id.name.as_str().to_string()),
        Expression::SequenceExpression(s) => s.expressions.last().and_then(ident_name),
        Expression::AssignmentExpression(a) => ident_name(&a.right),
        _ => None,
    }
}

fn ident_move_src(expr: &Expression<'_>) -> Option<String> {
    match peel(expr) {
        Expression::Identifier(id) => Some(id.name.as_str().to_string()),
        Expression::SequenceExpression(s) => s.expressions.last().and_then(ident_move_src),
        Expression::AssignmentExpression(a) => ident_move_src(&a.right),
        Expression::ConditionalExpression(c) => {
            let a = ident_move_src(&c.consequent);
            let b = ident_move_src(&c.alternate);
            match (a, b) {
                (Some(x), Some(y)) if x == y => Some(x),
                _ => None,
            }
        }
        Expression::LogicalExpression(b) if matches!(b.operator, LogicalOperator::And) => {
            ident_move_src(&b.right)
        }
        Expression::LogicalExpression(b) => {
            let left = ident_move_src(&b.left);
            let right = ident_move_src(&b.right);
            match (left, right) {
                (Some(x), Some(y)) if x == y => Some(x),
                (Some(x), _) => Some(x),
                (None, Some(y)) if !expr_is_call(&b.left) => Some(y),
                _ => None,
            }
        }
        _ => None,
    }
}

fn expr_is_call(expr: &Expression<'_>) -> bool {
    match peel(expr) {
        Expression::SequenceExpression(s) => s.expressions.last().is_some_and(expr_is_call),
        Expression::NewExpression(_) | Expression::TaggedTemplateExpression(_) => true,
        Expression::LogicalExpression(b) if matches!(b.operator, LogicalOperator::And) => {
            expr_is_call(&b.right)
        }
        Expression::LogicalExpression(b) => expr_is_call(&b.left) || expr_is_call(&b.right),
        Expression::ConditionalExpression(c) => {
            expr_is_call(&c.consequent) || expr_is_call(&c.alternate)
        }
        _ => as_call(expr).is_some(),
    }
}

fn atmd_binding_name(t: &oxc::ast::ast::AssignmentTargetMaybeDefault<'_>) -> Option<String> {
    match t {
        oxc::ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
            assignment_target_prefix(&d.binding)
        }
        other => other
            .as_assignment_target()
            .and_then(assignment_target_prefix),
    }
}

fn assignment_target_prefix(t: &oxc::ast::ast::AssignmentTarget<'_>) -> Option<String> {
    match t {
        oxc::ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
            Some(id.name.as_str().to_string())
        }
        oxc::ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
            let obj = callee_name(&m.object)?;
            Some(format!("{}.{}", obj, m.property.name.as_str()))
        }
        oxc::ast::ast::AssignmentTarget::TSAsExpression(e) => callee_name(&e.expression),
        oxc::ast::ast::AssignmentTarget::TSSatisfiesExpression(e) => callee_name(&e.expression),
        oxc::ast::ast::AssignmentTarget::TSNonNullExpression(e) => callee_name(&e.expression),
        oxc::ast::ast::AssignmentTarget::TSTypeAssertion(e) => callee_name(&e.expression),
        _ => None,
    }
}

fn peel<'a, 'b>(expr: &'b Expression<'a>) -> &'b Expression<'a> {
    match expr {
        Expression::ParenthesizedExpression(p) => peel(&p.expression),
        Expression::AwaitExpression(a) => peel(&a.argument),
        Expression::TSAsExpression(e) => peel(&e.expression),
        Expression::TSSatisfiesExpression(e) => peel(&e.expression),
        Expression::TSNonNullExpression(e) => peel(&e.expression),
        Expression::TSTypeAssertion(e) => peel(&e.expression),
        Expression::TSInstantiationExpression(e) => peel(&e.expression),
        _ => expr,
    }
}

fn as_call<'a, 'b>(expr: &'b Expression<'a>) -> Option<&'b CallExpression<'a>> {
    match peel(expr) {
        Expression::CallExpression(c) => Some(c),
        Expression::ChainExpression(c) => match &c.expression {
            oxc::ast::ast::ChainElement::CallExpression(call) => Some(call),
            _ => None,
        },
        Expression::AssignmentExpression(a) => as_call(&a.right),
        _ => None,
    }
}

fn case_falls_through(case: &oxc::ast::ast::SwitchCase<'_>) -> bool {
    if case.consequent.is_empty() {
        return false;
    }
    !stmts_exit(&case.consequent)
}

fn stmts_exit(stmts: &[Statement<'_>]) -> bool {
    stmts.iter().any(stmt_exits)
}

fn stmt_exits(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::BreakStatement(_)
        | Statement::ReturnStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::ContinueStatement(_) => true,
        Statement::BlockStatement(b) => stmts_exit(&b.body),
        Statement::LabeledStatement(l) => stmt_exits(&l.body),
        Statement::IfStatement(i) => {
            stmt_exits(&i.consequent) && i.alternate.as_ref().is_some_and(|a| stmt_exits(a))
        }
        _ => false,
    }
}

/// Dotted callee such as `fs.readFile` / `Buffer.from` / `Deno.readFile`.
fn callee_name(expr: &Expression<'_>) -> Option<String> {
    let expr = peel(expr);
    let name = match expr {
        Expression::Identifier(id) => id.name.as_str().to_string(),
        Expression::StaticMemberExpression(m) => {
            let obj = callee_name(&m.object)?;
            format!("{}.{}", obj, m.property.name.as_str())
        }
        Expression::ComputedMemberExpression(m) => {
            let obj = callee_name(&m.object)?;
            let key = string_prop_key(&m.expression)?;
            format!("{obj}.{key}")
        }
        Expression::AssignmentExpression(a) => return callee_name(&a.right),
        Expression::SequenceExpression(s) => return s.expressions.last().and_then(callee_name),
        Expression::LogicalExpression(b) => {
            return match b.operator {
                LogicalOperator::And => callee_name(&b.right),
                LogicalOperator::Or | LogicalOperator::Coalesce => {
                    callee_name(&b.left).or_else(|| callee_name(&b.right))
                }
            };
        }
        Expression::ConditionalExpression(c) => {
            return callee_name(&c.consequent).or_else(|| callee_name(&c.alternate));
        }
        Expression::ChainExpression(c) => match &c.expression {
            oxc::ast::ast::ChainElement::StaticMemberExpression(m) => {
                let obj = callee_name(&m.object)?;
                format!("{}.{}", obj, m.property.name.as_str())
            }
            oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                let obj = callee_name(&m.object)?;
                let key = string_prop_key(&m.expression)?;
                format!("{obj}.{key}")
            }
            _ => return None,
        },
        _ => return None,
    };
    Some(strip_global_prefix(&name))
}

fn callee_has_optional_access(expr: &Expression<'_>) -> bool {
    match peel(expr) {
        Expression::StaticMemberExpression(member) => {
            member.optional || callee_has_optional_access(&member.object)
        }
        Expression::ComputedMemberExpression(member) => {
            member.optional || callee_has_optional_access(&member.object)
        }
        Expression::PrivateFieldExpression(member) => {
            member.optional || callee_has_optional_access(&member.object)
        }
        _ => false,
    }
}

fn prop_key_name(key: &oxc::ast::ast::PropertyKey<'_>) -> Option<String> {
    match key {
        oxc::ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_string()),
        oxc::ast::ast::PropertyKey::PrivateIdentifier(_) => None,
        other => other.as_expression().and_then(string_prop_key),
    }
}

fn string_prop_key(expr: &Expression<'_>) -> Option<String> {
    match peel(expr) {
        Expression::AssignmentExpression(a) => return string_prop_key(&a.right),
        Expression::SequenceExpression(s) => return s.expressions.last().and_then(string_prop_key),
        Expression::LogicalExpression(b) => {
            return match b.operator {
                LogicalOperator::And => string_prop_key(&b.right),
                LogicalOperator::Or | LogicalOperator::Coalesce => {
                    string_prop_key(&b.left).or_else(|| string_prop_key(&b.right))
                }
            };
        }
        Expression::ConditionalExpression(c) => {
            let a = string_prop_key(&c.consequent)?;
            let b = string_prop_key(&c.alternate)?;
            return (a == b).then_some(a);
        }
        Expression::StringLiteral(s) => Some(s.value.as_str().to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t.quasis.first().map(|q| {
            q.value
                .cooked
                .as_ref()
                .map(|c| c.as_str().to_string())
                .unwrap_or_else(|| q.value.raw.as_str().to_string())
        }),
        Expression::NumericLiteral(n) => Some(numeric_prop_key(n.value)),
        Expression::BigIntLiteral(n) => Some(n.value.as_str().to_string()),
        _ => None,
    }
}

fn numeric_prop_key(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

enum FlatArrayEl<'a> {
    Expr(&'a Expression<'a>),
    Spread(&'a Expression<'a>),
    Hole,
}

fn flatten_array_elements<'a>(
    elements: &'a [oxc::ast::ast::ArrayExpressionElement<'a>],
    out: &mut Vec<FlatArrayEl<'a>>,
) {
    for el in elements {
        match el {
            oxc::ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                flatten_spread_arg(&s.argument, out);
            }
            oxc::ast::ast::ArrayExpressionElement::Elision(_) => out.push(FlatArrayEl::Hole),
            other => {
                if let Some(e) = other.as_expression() {
                    out.push(FlatArrayEl::Expr(e));
                }
            }
        }
    }
}

fn known_flat_len(expr: &Expression<'_>) -> Option<usize> {
    match peel(expr) {
        Expression::AssignmentExpression(a) => known_flat_len(&a.right),
        Expression::ArrayExpression(arr) => {
            let mut flat = Vec::new();
            flatten_array_elements(&arr.elements, &mut flat);
            if flat.iter().any(|e| matches!(e, FlatArrayEl::Spread(_))) {
                None
            } else {
                Some(flat.len())
            }
        }
        Expression::SequenceExpression(s) => s.expressions.last().and_then(|e| known_flat_len(e)),
        Expression::LogicalExpression(b) => {
            let a = known_flat_len(&b.left);
            let c = known_flat_len(&b.right);
            match (a, c) {
                (Some(x), Some(y)) if x == y => Some(x),
                (Some(x), None) if is_scalar_literal(&b.right) => Some(x),
                (None, Some(y)) if is_scalar_literal(&b.left) => Some(y),
                _ => None,
            }
        }
        Expression::ConditionalExpression(c) => {
            let a = known_flat_len(&c.consequent)?;
            let b = known_flat_len(&c.alternate)?;
            (a == b).then_some(a)
        }
        _ => None,
    }
}

fn is_scalar_literal(expr: &Expression<'_>) -> bool {
    match peel(expr) {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,
        Expression::Identifier(id) => id.name.as_str() == "undefined",
        Expression::UnaryExpression(u) if matches!(u.operator, UnaryOperator::Void) => true,
        _ => false,
    }
}

fn flatten_spread_arg<'a>(arg: &'a Expression<'a>, out: &mut Vec<FlatArrayEl<'a>>) {
    match peel(arg) {
        Expression::AssignmentExpression(a) => flatten_spread_arg(&a.right, out),
        Expression::ArrayExpression(arr) => flatten_array_elements(&arr.elements, out),
        Expression::SequenceExpression(s) => {
            if let Some(e) = s.expressions.last() {
                flatten_spread_arg(e, out);
            }
        }
        Expression::YieldExpression(y) => {
            if let Some(a) = &y.argument {
                flatten_spread_arg(a, out);
            }
        }
        _ => out.push(FlatArrayEl::Spread(arg)),
    }
}

fn collect_fn_init_offs(init: &Expression<'_>, offs: &mut Vec<u32>) {
    match peel(init) {
        Expression::FunctionExpression(f) => {
            offs.push(f.span.start);
            if let Some(id) = &f.id {
                offs.push(id.span.start);
            }
        }
        Expression::ArrowFunctionExpression(a) => offs.push(a.span.start),
        Expression::AssignmentExpression(a) => collect_fn_init_offs(&a.right, offs),
        Expression::SequenceExpression(s) => {
            if let Some(e) = s.expressions.last() {
                collect_fn_init_offs(e, offs);
            }
        }
        Expression::LogicalExpression(b) => {
            collect_fn_init_offs(&b.left, offs);
            collect_fn_init_offs(&b.right, offs);
        }
        Expression::ConditionalExpression(c) => {
            collect_fn_init_offs(&c.consequent, offs);
            collect_fn_init_offs(&c.alternate, offs);
        }
        _ => {}
    }
}

fn new_instance_type(expr: &Expression<'_>) -> Option<String> {
    match peel(expr) {
        Expression::AssignmentExpression(a) => new_instance_type(&a.right),
        Expression::SequenceExpression(s) => {
            s.expressions.last().and_then(|e| new_instance_type(e))
        }
        Expression::ConditionalExpression(c) => {
            let a = new_instance_type(&c.consequent)?;
            let b = new_instance_type(&c.alternate)?;
            (a == b).then_some(a)
        }
        Expression::LogicalExpression(b) => match b.operator {
            LogicalOperator::And => new_instance_type(&b.right),
            LogicalOperator::Or | LogicalOperator::Coalesce => {
                new_instance_type(&b.left).or_else(|| new_instance_type(&b.right))
            }
        },
        Expression::NewExpression(n) => class_name_of_callee(&n.callee),
        _ => None,
    }
}

fn class_name_of_callee(expr: &Expression<'_>) -> Option<String> {
    if let Some(n) = callee_name(expr) {
        return Some(n);
    }
    match peel(expr) {
        Expression::ClassExpression(c) => c.id.as_ref().map(|id| id.name.as_str().to_string()),
        Expression::AssignmentExpression(a) => class_name_of_callee(&a.right),
        Expression::SequenceExpression(s) => {
            s.expressions.last().and_then(|e| class_name_of_callee(e))
        }
        Expression::ConditionalExpression(c) => {
            let a = class_name_of_callee(&c.consequent)?;
            let b = class_name_of_callee(&c.alternate)?;
            (a == b).then_some(a)
        }
        Expression::LogicalExpression(b) => match b.operator {
            LogicalOperator::And => class_name_of_callee(&b.right),
            LogicalOperator::Or | LogicalOperator::Coalesce => {
                class_name_of_callee(&b.left).or_else(|| class_name_of_callee(&b.right))
            }
        },
        _ => None,
    }
}

fn instance_member_object<'a, 'b>(call: &'b CallExpression<'a>) -> Option<&'b Expression<'a>> {
    match &call.callee {
        Expression::StaticMemberExpression(m) => Some(&m.object),
        Expression::ComputedMemberExpression(m) => Some(&m.object),
        _ => None,
    }
}

fn strip_global_prefix(name: &str) -> String {
    for prefix in ["globalThis.", "window.", "global."] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    name.to_string()
}
