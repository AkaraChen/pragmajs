//! Austral-style linearity / borrow checker over an oxc AST.

use crate::annot::{
    parse_own_comment, AttachedOwn, BorrowMode, FnSig, OwnDirective, OwnType,
};
use crate::{Diagnostic, RuleKind};
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    ArrowFunctionBody, ArrowFunctionExpression, BindingPattern, CallExpression, Expression,
    ForStatementInit, Function, FunctionBody, Program, Statement, UnaryOperator,
    VariableDeclaration,
};
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType, Span};
use std::collections::HashMap;

pub fn check_source(filename: &str, source: &str) -> Vec<Diagnostic> {
    let allocator = Allocator::new();
    let source_type = SourceType::from_path(filename).unwrap_or_else(|_| {
        if filename.ends_with(".ts") || filename.ends_with(".tsx") || filename.ends_with(".mts") {
            SourceType::ts()
        } else {
            SourceType::mjs()
        }
    });
    let ret = Parser::new(&allocator, source, source_type).parse();
    check_program(filename, source, &ret.program)
}

fn check_program(path: &str, source: &str, program: &Program<'_>) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut annots: HashMap<u32, Vec<AttachedOwn>> = HashMap::new();
    for comment in program.comments.iter() {
        let content = comment.content_span().source_text(source);
        if let Some(att) = parse_own_comment(
            path,
            comment.attached_to,
            comment.span.start,
            comment.span.end,
            content,
            &mut diags,
        ) {
            annots.entry(comment.attached_to).or_default().push(att);
        }
    }

    let mut file = FileCtx {
        path: path.to_string(),
        source,
        annots,
        sigs: HashMap::new(),
    };
    file.collect_sigs_program(program);
    let mut checker = Checker {
        file: &file,
        diags,
        tbl: HashMap::new(),
        scopes: Vec::new(),
        suppress_consume: None,
        loop_depth: 0,
    };
    checker.check_program(program);
    checker.diags
}

struct FileCtx<'a> {
    path: String,
    #[allow(dead_code)]
    source: &'a str,
    annots: HashMap<u32, Vec<AttachedOwn>>,
    sigs: HashMap<String, FnSig>,
}

impl FileCtx<'_> {
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
                self.collect_sigs_stmt(&i.consequent, None);
                if let Some(a) = &i.alternate {
                    self.collect_sigs_stmt(a, None);
                }
            }
            Statement::WhileStatement(w) => self.collect_sigs_stmt(&w.body, None),
            Statement::DoWhileStatement(w) => self.collect_sigs_stmt(&w.body, None),
            Statement::ForStatement(f) => self.collect_sigs_stmt(&f.body, None),
            Statement::ForInStatement(f) => self.collect_sigs_stmt(&f.body, None),
            Statement::ForOfStatement(f) => self.collect_sigs_stmt(&f.body, None),
            Statement::SwitchStatement(s) => {
                for c in &s.cases {
                    for st in &c.consequent {
                        self.collect_sigs_stmt(st, None);
                    }
                }
            }
            Statement::TryStatement(t) => {
                for s in &t.block.body {
                    self.collect_sigs_stmt(s, None);
                }
                if let Some(h) = &t.handler {
                    for s in &h.body.body {
                        self.collect_sigs_stmt(s, None);
                    }
                }
            }
            Statement::LabeledStatement(l) => self.collect_sigs_stmt(&l.body, None),
            _ => {}
        }
    }

    fn collect_sigs_decl(&mut self, decl: &oxc::ast::ast::Declaration<'_>, extra: Option<u32>) {
        match decl {
            oxc::ast::ast::Declaration::FunctionDeclaration(f) => self.collect_fn(f, extra),
            oxc::ast::ast::Declaration::VariableDeclaration(v) => self.collect_var(v, extra),
            _ => {}
        }
    }

    fn collect_sigs_export_default(&mut self, e: &oxc::ast::ast::ExportDefaultDeclaration<'_>) {
        match &e.declaration {
            oxc::ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f)
            | oxc::ast::ast::ExportDefaultDeclarationKind::FunctionExpression(f) => {
                self.collect_fn(f, Some(e.span.start));
            }
            _ => {}
        }
    }

    fn collect_fn(&mut self, func: &Function<'_>, extra: Option<u32>) {
        let mut offs = vec![func.span.start];
        if let Some(e) = extra {
            offs.push(e);
        }
        if let Some(id) = &func.id {
            offs.push(id.span.start);
        }
        if let Some(sig) = self.type_sig_at(&offs) {
            if let Some(name) = func.name() {
                self.sigs.insert(name.as_str().to_string(), sig);
            }
        }
        if let Some(body) = &func.body {
            for s in &body.statements {
                self.collect_sigs_stmt(s, None);
            }
        }
    }

    fn collect_var(&mut self, decl: &VariableDeclaration<'_>, extra: Option<u32>) {
        for d in &decl.declarations {
            let mut offs = vec![decl.span.start, d.span.start];
            if let Some(e) = extra {
                offs.push(e);
            }
            if let Some(sig) = self.type_sig_at(&offs) {
                if let Some(name) = ident_of_pattern(&d.id) {
                    self.sigs.insert(name, sig);
                }
            }
            if let Some(init) = &d.init {
                self.collect_sigs_expr(init);
            }
        }
    }

    fn collect_sigs_expr(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::FunctionExpression(f) => self.collect_fn(f, None),
            Expression::ArrowFunctionExpression(a) => self.collect_sigs_arrow(a),
            Expression::ParenthesizedExpression(p) => self.collect_sigs_expr(&p.expression),
            _ => {}
        }
    }

    fn collect_sigs_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>) {
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
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnKind {
    Unique,
    Affine,
    #[allow(dead_code)]
    Copy,
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
    scopes: Vec<Vec<String>>,
    suppress_consume: Option<String>,
    loop_depth: u32,
}

impl Checker<'_> {
    fn emit(&mut self, offset: u32, kind: RuleKind, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            path: self.file.path.clone(),
            offset,
            kind,
            message: message.into(),
        });
    }

    fn check_program(&mut self, program: &Program<'_>) {
        for stmt in &program.body {
            self.check_stmt(stmt);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let names = self.scopes.pop().unwrap_or_default();
        for name in names.into_iter().rev() {
            self.remove_var(&name);
        }
    }

    fn add_var(&mut self, name: String, entry: VarEntry) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.clone());
        }
        self.tbl.insert(name, entry);
    }

    fn remove_var(&mut self, name: &str) {
        let Some(entry) = self.tbl.remove(name) else {
            return;
        };
        match entry.kind {
            OwnKind::Unique if entry.state != VarState::Consumed => {
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
                        && matches!(
                            e.state,
                            VarState::BorrowedRead | VarState::BorrowedWrite
                        )
                    {
                        e.state = VarState::Unconsumed;
                    }
                }
                OwnKind::RefWrite => {
                    e.write_borrows = e.write_borrows.saturating_sub(1);
                    if e.read_borrows == 0
                        && e.write_borrows == 0
                        && matches!(
                            e.state,
                            VarState::BorrowedRead | VarState::BorrowedWrite
                        )
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
            Statement::VariableDeclaration(v) => self.check_var_decl(v),
            Statement::FunctionDeclaration(f) => self.check_function(f, &[f.span.start]),
            Statement::IfStatement(i) => {
                self.check_expr(&i.test, i.test.span().start);
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
                self.check_expr(&w.test, w.test.span().start);
                self.loop_depth += 1;
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
                self.loop_depth -= 1;
                self.check_expr(&w.test, w.test.span().start);
            }
            Statement::ForStatement(f) => {
                self.push_scope();
                if let Some(init) = &f.init {
                    match init {
                        ForStatementInit::VariableDeclaration(v) => self.check_var_decl(v),
                        other => {
                            if let Some(e) = other.as_expression() {
                                self.check_expr(e, e.span().start);
                            }
                        }
                    }
                }
                if let Some(test) = &f.test {
                    self.check_expr(test, test.span().start);
                }
                self.loop_depth += 1;
                if let Some(upd) = &f.update {
                    self.check_expr(upd, upd.span().start);
                }
                self.check_stmt(&f.body);
                self.loop_depth -= 1;
                self.pop_scope();
            }
            Statement::ForInStatement(f) => {
                self.check_expr(&f.right, f.right.span().start);
                self.loop_depth += 1;
                self.push_scope();
                self.check_stmt(&f.body);
                self.pop_scope();
                self.loop_depth -= 1;
            }
            Statement::ForOfStatement(f) => {
                self.check_expr(&f.right, f.right.span().start);
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
                }
                self.require_consumed_uniques(r.span.start);
            }
            Statement::ThrowStatement(t) => {
                self.check_expr(&t.argument, t.argument.span().start);
                self.require_consumed_uniques(t.span.start);
            }
            Statement::ExpressionStatement(e) => {
                self.check_unmapped_eval(&e.expression);
                self.check_expr(&e.expression, e.span.start);
                self.check_discard(&e.expression);
            }
            Statement::SwitchStatement(s) => {
                self.check_expr(&s.discriminant, s.discriminant.span().start);
                let base = self.tbl.clone();
                let mut tables: Vec<HashMap<String, VarEntry>> = Vec::new();
                let has_default = s.cases.iter().any(|c| c.test.is_none());
                for case in &s.cases {
                    self.tbl = base.clone();
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
                let base = self.tbl.clone();
                self.push_scope();
                for s in &t.block.body {
                    self.check_stmt(s);
                }
                self.pop_scope();
                let try_tbl = self.tbl.clone();
                if let Some(h) = &t.handler {
                    self.tbl = base;
                    self.push_scope();
                    for s in &h.body.body {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
                    let catch_tbl = self.tbl.clone();
                    self.tables_consistent("a try/catch", &try_tbl, &catch_tbl, t.span.start);
                    self.tbl = try_tbl;
                }
            }
            Statement::WithStatement(w) => {
                self.emit(
                    w.span.start,
                    RuleKind::UnmappedConstruct,
                    "`with` statements are not mapped from Austral linearity",
                );
                self.check_stmt(&w.body);
            }
            Statement::LabeledStatement(l) => self.check_stmt(&l.body),
            Statement::ExportDeclaration(e) => self.check_decl(&e.declaration, e.span.start),
            Statement::ExportDefaultDeclaration(e) => {
                match &e.declaration {
                    oxc::ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f)
                    | oxc::ast::ast::ExportDefaultDeclarationKind::FunctionExpression(f) => {
                        self.check_function(f, &[f.span.start, e.span.start]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn check_decl(&mut self, decl: &oxc::ast::ast::Declaration<'_>, extra: u32) {
        match decl {
            oxc::ast::ast::Declaration::FunctionDeclaration(f) => {
                self.check_function(f, &[f.span.start, extra]);
            }
            oxc::ast::ast::Declaration::VariableDeclaration(v) => self.check_var_decl(v),
            _ => {}
        }
    }

    fn apply_stmt_directives(&mut self, stmt: &Statement<'_>) {
        // Variable declarations apply let/borrow/clone themselves.
        if matches!(stmt, Statement::VariableDeclaration(_)) {
            return;
        }
        let start = stmt.span().start;
        let dirs: Vec<OwnDirective> = self.file.dirs_at(start).into_iter().cloned().collect();
        for d in dirs {
            match d {
                OwnDirective::Drop { name } => {
                    self.force_consume(&name, start);
                }
                OwnDirective::Borrow {
                    owner,
                    alias,
                    mode,
                    ..
                } => {
                    self.begin_borrow(&owner, &alias, mode, start);
                }
                OwnDirective::Clone { owner, alias } => {
                    self.do_clone(&owner, &alias, start);
                }
                OwnDirective::Let { name, ty } => {
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
            },
        );
    }

    fn add_from_type(&mut self, name: &str, ty: &OwnType, span: u32) {
        let kind = match ty {
            OwnType::Unique(_) => OwnKind::Unique,
            OwnType::Affine(_) => OwnKind::Affine,
            OwnType::Copy(_) => return,
            OwnType::RefRead(_) => OwnKind::RefRead,
            OwnType::RefWrite(_) => OwnKind::RefWrite,
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
            },
        );
    }

    fn force_consume(&mut self, name: &str, span: u32) {
        let mut apps = Apps::default();
        apps.consumed = 1;
        self.apply_apps(name, apps, span);
    }

    fn check_var_decl(&mut self, decl: &VariableDeclaration<'_>) {
        let decl_dirs: Vec<OwnDirective> = self
            .file
            .dirs_at(decl.span.start)
            .into_iter()
            .cloned()
            .collect();
        for d in &decl.declarations {
            let mut dirs = decl_dirs.clone();
            dirs.extend(self.file.dirs_at(d.span.start).into_iter().cloned());
            let name = ident_of_pattern(&d.id);
            let borrow = dirs.iter().find_map(|x| match x {
                OwnDirective::Borrow {
                    owner,
                    alias,
                    mode,
                    ..
                } => Some((owner.clone(), alias.clone(), *mode)),
                _ => None,
            });
            let clone = dirs.iter().find_map(|x| match x {
                OwnDirective::Clone { owner, alias } => Some((owner.clone(), alias.clone())),
                _ => None,
            });
            let let_ty = dirs.iter().find_map(|x| match x {
                OwnDirective::Let { name, ty } => Some((name.clone(), ty.clone())),
                _ => None,
            });

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
                let src_name = ident_name(init);
                let src_kind = src_name.as_ref().and_then(|n| self.tbl.get(n).map(|e| e.kind));
                self.check_expr(init, init.span().start);
                if let Some(n) = &name {
                    if let Some((let_name, ty)) = &let_ty {
                        if let_name == n {
                            self.add_from_type(n, ty, d.span.start);
                            continue;
                        }
                    }
                    if let Some(kind) = src_kind {
                        if matches!(kind, OwnKind::Unique | OwnKind::Affine) {
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
                        continue;
                    }
                    if let Some((_, ty)) = &let_ty {
                        self.add_from_type(n, ty, d.span.start);
                    }
                }
            } else if let Some(n) = &name {
                if let Some((let_name, ty)) = &let_ty {
                    if let_name == n {
                        self.add_from_type(n, ty, d.span.start);
                    }
                }
            }

            if let Some(init) = &d.init {
                match init {
                    Expression::FunctionExpression(f) => {
                        let offs = [decl.span.start, d.span.start, f.span.start];
                        if let Some(sig) = self.file.type_sig_at(&offs) {
                            self.check_function_with_sig(f, &sig);
                        }
                    }
                    Expression::ArrowFunctionExpression(a) => {
                        let offs = [decl.span.start, d.span.start, a.span.start];
                        if let Some(sig) = self.file.type_sig_at(&offs) {
                            self.check_arrow(a, &sig);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn check_function(&mut self, func: &Function<'_>, extra: &[u32]) {
        let mut offs: Vec<u32> = extra.to_vec();
        offs.push(func.span.start);
        if let Some(id) = &func.id {
            offs.push(id.span.start);
        }
        let Some(sig) = self.file.type_sig_at(&offs) else {
            if let Some(body) = &func.body {
                self.scan_unmapped_body(body);
            }
            return;
        };
        self.check_function_with_sig(func, &sig);
    }

    fn check_function_with_sig(&mut self, func: &Function<'_>, sig: &FnSig) {
        let saved_tbl = self.tbl.clone();
        let saved_scopes = self.scopes.clone();
        let saved_depth = self.loop_depth;
        self.tbl.clear();
        self.scopes.clear();
        self.loop_depth = 0;
        self.push_scope();
        let params: Vec<(String, u32)> = func
            .params
            .items
            .iter()
            .filter_map(|p| {
                ident_of_pattern(&p.pattern).map(|n| (n, p.span.start))
            })
            .collect();
        for (i, (pname, pspan)) in params.iter().enumerate() {
            if let Some((_, ty)) = sig.params.get(i) {
                self.add_from_type(pname, ty, *pspan);
            }
        }
        if let Some(body) = &func.body {
            for s in &body.statements {
                self.check_stmt(s);
            }
        }
        self.pop_scope();
        self.tbl = saved_tbl;
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
    }

    fn check_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>, sig: &FnSig) {
        let saved_tbl = self.tbl.clone();
        let saved_scopes = self.scopes.clone();
        let saved_depth = self.loop_depth;
        self.tbl.clear();
        self.scopes.clear();
        self.loop_depth = 0;
        self.push_scope();
        let params: Vec<(String, u32)> = arrow
            .params
            .items
            .iter()
            .filter_map(|p| ident_of_pattern(&p.pattern).map(|n| (n, p.span.start)))
            .collect();
        for (i, (pname, pspan)) in params.iter().enumerate() {
            if let Some((_, ty)) = sig.params.get(i) {
                self.add_from_type(pname, ty, *pspan);
            }
        }
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
                }
            }
        }
        self.pop_scope();
        self.tbl = saved_tbl;
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
    }

    fn require_consumed_uniques(&mut self, span: u32) {
        let names: Vec<_> = self.tbl.keys().cloned().collect();
        for name in names {
            if let Some(e) = self.tbl.get(&name) {
                if e.kind == OwnKind::Unique && e.state != VarState::Consumed {
                    self.emit(
                        span,
                        RuleKind::UniqueForget,
                        format!("unique value `{name}` is not consumed"),
                    );
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
        if self.scopes.is_empty() {
            return;
        }
        if let Some(OwnType::Unique(_)) = self.call_return_type(expr) {
            self.emit(
                expr.span().start,
                RuleKind::UniqueForget,
                "unique value discarded without being bound or consumed",
            );
        }
    }

    fn call_return_type(&self, expr: &Expression<'_>) -> Option<OwnType> {
        let Expression::CallExpression(call) = expr else {
            return None;
        };
        let callee = ident_name(&call.callee)?;
        self.file.sigs.get(&callee).map(|s| s.ret.clone())
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
                if m.property.name.as_str() == "__proto__" || m.property.name.as_str() == "prototype"
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
            }
            _ => {}
        }
    }

    fn check_nested_fn_captures(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::FunctionExpression(f) => {
                if let Some(body) = &f.body {
                    self.report_captures_in_body(body);
                }
            }
            Expression::ArrowFunctionExpression(a) => {
                if let Some(body) = a.body.as_function_body() {
                    self.report_captures_in_body(body);
                }
            }
            _ => {}
        }
    }

    fn report_captures_in_body(&mut self, body: &FunctionBody<'_>) {
        let owned: Vec<String> = self.tbl.keys().cloned().collect();
        if owned.is_empty() {
            return;
        }
        for stmt in &body.statements {
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
            Statement::BlockStatement(b) => {
                for s in &b.body {
                    self.report_captures_stmt(s, owned);
                }
            }
            Statement::VariableDeclaration(v) => {
                for d in &v.declarations {
                    if let Some(i) = &d.init {
                        self.report_captures_expr(i, owned);
                    }
                }
            }
            _ => {}
        }
    }

    fn report_captures_expr(&mut self, expr: &Expression<'_>, owned: &[String]) {
        if let Some(n) = ident_name(expr) {
            if owned.iter().any(|o| o == &n) {
                self.emit(
                    expr.span().start,
                    RuleKind::UnmappedConstruct,
                    format!("nested function capturing owned value `{n}` is not mapped"),
                );
            }
        }
        match expr {
            Expression::CallExpression(c) => {
                self.report_captures_expr(&c.callee, owned);
                for a in &c.arguments {
                    if let Some(e) = a.as_expression() {
                        self.report_captures_expr(e, owned);
                    }
                }
            }
            Expression::StaticMemberExpression(m) => self.report_captures_expr(&m.object, owned),
            Expression::BinaryExpression(b) => {
                self.report_captures_expr(&b.left, owned);
                self.report_captures_expr(&b.right, owned);
            }
            Expression::ParenthesizedExpression(p) => {
                self.report_captures_expr(&p.expression, owned)
            }
            _ => {}
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
                let mut a = self.count(&n.callee, name);
                for arg in &n.arguments {
                    if let Some(e) = arg.as_expression() {
                        a = a.merge(self.count(e, name));
                    }
                }
                a
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
            Expression::AssignmentExpression(a) => {
                self.count(&a.right, name)
            }
            Expression::UnaryExpression(u) => self.count(&u.argument, name),
            Expression::UpdateExpression(u) => {
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
            Expression::BinaryExpression(b) => {
                self.count(&b.left, name).merge(self.count(&b.right, name))
            }
            Expression::LogicalExpression(b) => {
                self.count(&b.left, name).merge(self.count(&b.right, name))
            }
            Expression::ConditionalExpression(c) => self
                .count(&c.test, name)
                .merge(self.count(&c.consequent, name))
                .merge(self.count(&c.alternate, name)),
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
                    if let Some(e) = el.as_expression() {
                        a = a.merge(self.count(e, name));
                    }
                }
                a
            }
            Expression::ObjectExpression(obj) => {
                let mut a = Apps::default();
                for p in &obj.properties {
                    if let oxc::ast::ast::ObjectPropertyKind::ObjectProperty(p) = p {
                        a = a.merge(self.count(&p.value, name));
                    }
                }
                a
            }
            Expression::ParenthesizedExpression(p) => self.count(&p.expression, name),
            Expression::ChainExpression(c) => match &c.expression {
                oxc::ast::ast::ChainElement::CallExpression(call) => self.count_call(call, name),
                oxc::ast::ast::ChainElement::ComputedMemberExpression(m) => {
                    self.count(&m.object, name).merge(self.count(&m.expression, name))
                }
                oxc::ast::ast::ChainElement::StaticMemberExpression(m) => self.count(&m.object, name),
                oxc::ast::ast::ChainElement::PrivateFieldExpression(m) => self.count(&m.object, name),
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
                self.count(&t.tag, name).merge({
                    let mut a = Apps::default();
                    for e in &t.quasi.expressions {
                        a = a.merge(self.count(e, name));
                    }
                    a
                })
            }
            Expression::TSAsExpression(e) => self.count(&e.expression, name),
            Expression::TSSatisfiesExpression(e) => self.count(&e.expression, name),
            Expression::TSNonNullExpression(e) => self.count(&e.expression, name),
            Expression::TSTypeAssertion(e) => self.count(&e.expression, name),
            Expression::TSInstantiationExpression(e) => self.count(&e.expression, name),
            _ => Apps::default(),
        }
    }

    fn count_call(&self, call: &CallExpression<'_>, name: &str) -> Apps {
        let mut apps = self.count(&call.callee, name);
        let callee = ident_name(&call.callee);
        let sig = callee.as_ref().and_then(|n| self.file.sigs.get(n));
        for (i, arg) in call.arguments.iter().enumerate() {
            let Some(expr) = arg.as_expression() else {
                continue;
            };
            let mode = self.arg_mode(expr, sig, i);
            apps = apps.merge(self.count_arg(expr, name, mode));
        }
        apps
    }

    fn arg_mode(&self, expr: &Expression<'_>, sig: Option<&FnSig>, i: usize) -> ArgMode {
        let start = expr.span().start;
        for d in self.file.dirs_at(start) {
            match d {
                OwnDirective::Shorthand(BorrowMode::Read) => return ArgMode::Read,
                OwnDirective::Shorthand(BorrowMode::Write) => return ArgMode::Write,
                _ => {}
            }
        }
        if let Some(sig) = sig {
            if let Some((_, ty)) = sig.params.get(i) {
                return match ty {
                    OwnType::RefRead(_) => ArgMode::Read,
                    OwnType::RefWrite(_) => ArgMode::Write,
                    OwnType::Copy(_) => ArgMode::Path,
                    _ => ArgMode::Consume,
                };
            }
        }
        ArgMode::Consume
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
        let Some(entry) = self.tbl.get(name).cloned() else {
            return;
        };
        if matches!(
            entry.kind,
            OwnKind::Copy | OwnKind::RefRead | OwnKind::RefWrite
        ) {
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
                    format!("`{name}` is consumed and also borrowed or used in the same expression"),
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
                self.emit(
                    span,
                    kind,
                    format!("`{name}` has already been consumed"),
                );
            }
        }
    }

    fn consume_once(&mut self, name: &str, span: u32) {
        let Some(entry) = self.tbl.get(name) else {
            return;
        };
        if self.loop_depth != entry.loop_depth {
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
}

#[derive(Clone, Copy)]
enum ArgMode {
    Consume,
    Read,
    Write,
    Path,
}

fn ident_of_pattern(pat: &BindingPattern<'_>) -> Option<String> {
    pat.get_identifier_name().map(|n| n.as_str().to_string())
}

fn ident_name(expr: &Expression<'_>) -> Option<String> {
    match expr {
        Expression::Identifier(id) => Some(id.name.as_str().to_string()),
        Expression::ParenthesizedExpression(p) => ident_name(&p.expression),
        Expression::TSAsExpression(e) => ident_name(&e.expression),
        Expression::TSNonNullExpression(e) => ident_name(&e.expression),
        _ => None,
    }
}

#[allow(dead_code)]
fn _span(s: Span) -> u32 {
    s.start
}

#[allow(dead_code)]
fn _void(op: UnaryOperator) -> bool {
    matches!(op, UnaryOperator::Void)
}
