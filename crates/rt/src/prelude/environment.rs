use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ExportAllDeclaration, ExportFromDeclaration, Expression, ImportDeclaration, ImportExpression,
};
use oxc_ast_visit::{
    Visit,
    walk::{
        walk_export_all_declaration, walk_export_from_declaration, walk_import_declaration,
        walk_import_expression,
    },
};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use std::{collections::BTreeSet, error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Environment {
    #[default]
    Auto,
    Ecmascript,
    Browser,
    Node,
    Deno,
    Bun,
}

impl Environment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ecmascript => "ecmascript",
            Self::Browser => "browser",
            Self::Node => "node",
            Self::Deno => "deno",
            Self::Bun => "bun",
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvidenceKind {
    Import,
    Global,
}

impl fmt::Display for EvidenceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Import => formatter.write_str("import"),
            Self::Global => formatter.write_str("global"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvironmentEvidence {
    pub environment: Environment,
    pub kind: EvidenceKind,
    pub marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentError {
    Parse {
        diagnostics: Vec<String>,
    },
    Conflict {
        candidates: Vec<Environment>,
        evidence: Vec<EnvironmentEvidence>,
    },
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { diagnostics } => {
                write!(
                    formatter,
                    "cannot detect environment from invalid JavaScript"
                )?;
                if let Some(first) = diagnostics.first() {
                    write!(formatter, ": {first}")?;
                }
                Ok(())
            }
            Self::Conflict {
                candidates,
                evidence,
            } => {
                let candidates = candidates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    formatter,
                    "conflicting environment markers for {candidates}"
                )?;
                for item in evidence {
                    write!(
                        formatter,
                        "; {} {} '{}'",
                        item.environment, item.kind, item.marker
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl Error for EnvironmentError {}

/// Detect a single runtime using syntax-aware import and unbound-global
/// evidence. Node compatibility markers are accepted inside Deno and Bun.
pub fn detect_environment(source: &str) -> Result<Environment, EnvironmentError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::default().with_module(true))
        .with_options(ParseOptions::default())
        .parse();
    if !parsed.diagnostics.is_empty() {
        return Err(EnvironmentError::Parse {
            diagnostics: parsed
                .diagnostics
                .iter()
                .map(|diagnostic| format!("{diagnostic:?}"))
                .collect(),
        });
    }

    let mut imports = ModuleSourceCollector::default();
    imports.visit_program(&parsed.program);

    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let globals: BTreeSet<String> = semantic
        .scoping()
        .root_unresolved_references()
        .keys()
        .map(|name| name.to_string())
        .collect();

    let mut evidence = BTreeSet::new();
    for specifier in imports.sources {
        if let Some(environment) = environment_for_import(&specifier) {
            evidence.insert(EnvironmentEvidence {
                environment,
                kind: EvidenceKind::Import,
                marker: specifier,
            });
        }
    }
    for name in globals {
        if let Some(environment) = environment_for_global(&name) {
            evidence.insert(EnvironmentEvidence {
                environment,
                kind: EvidenceKind::Global,
                marker: name,
            });
        }
    }

    let mut candidates: BTreeSet<Environment> =
        evidence.iter().map(|item| item.environment).collect();
    // Both runtimes intentionally implement the Node compatibility surface.
    if candidates.contains(&Environment::Bun) || candidates.contains(&Environment::Deno) {
        candidates.remove(&Environment::Node);
    }

    match candidates.len() {
        0 => Ok(Environment::Ecmascript),
        1 => Ok(*candidates.first().expect("one candidate")),
        _ => {
            let candidates: Vec<_> = candidates.into_iter().collect();
            let evidence = evidence
                .into_iter()
                .filter(|item| candidates.contains(&item.environment))
                .collect();
            Err(EnvironmentError::Conflict {
                candidates,
                evidence,
            })
        }
    }
}

#[derive(Default)]
struct ModuleSourceCollector {
    sources: BTreeSet<String>,
}

impl ModuleSourceCollector {
    fn insert(&mut self, source: &str) {
        self.sources.insert(source.to_string());
    }
}

impl<'a> Visit<'a> for ModuleSourceCollector {
    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        self.insert(declaration.source.value.as_str());
        walk_import_declaration(self, declaration);
    }

    fn visit_import_expression(&mut self, expression: &ImportExpression<'a>) {
        if let Expression::StringLiteral(source) = &expression.source {
            self.insert(source.value.as_str());
        }
        walk_import_expression(self, expression);
    }

    fn visit_export_from_declaration(&mut self, declaration: &ExportFromDeclaration<'a>) {
        self.insert(declaration.source.value.as_str());
        walk_export_from_declaration(self, declaration);
    }

    fn visit_export_all_declaration(&mut self, declaration: &ExportAllDeclaration<'a>) {
        self.insert(declaration.source.value.as_str());
        walk_export_all_declaration(self, declaration);
    }
}

fn environment_for_global(name: &str) -> Option<Environment> {
    match name {
        "Bun" => Some(Environment::Bun),
        "Deno" => Some(Environment::Deno),
        "document" | "window" | "HTMLElement" | "customElements" => Some(Environment::Browser),
        "process" | "Buffer" | "global" | "__dirname" | "__filename" | "require" | "module"
        | "exports" => Some(Environment::Node),
        _ => None,
    }
}

fn environment_for_import(specifier: &str) -> Option<Environment> {
    if specifier.starts_with("bun:") {
        return Some(Environment::Bun);
    }
    if specifier.starts_with("jsr:")
        || specifier.starts_with("deno:")
        || specifier.starts_with("npm:")
        || specifier.starts_with("ext:")
    {
        return Some(Environment::Deno);
    }
    if specifier.starts_with("node:") || is_bare_node_builtin(specifier) {
        return Some(Environment::Node);
    }
    None
}

fn is_bare_node_builtin(specifier: &str) -> bool {
    let root = specifier.split('/').next().unwrap_or(specifier);
    matches!(
        root,
        "assert"
            | "async_hooks"
            | "buffer"
            | "child_process"
            | "cluster"
            | "console"
            | "crypto"
            | "diagnostics_channel"
            | "dns"
            | "events"
            | "fs"
            | "http"
            | "http2"
            | "https"
            | "module"
            | "net"
            | "os"
            | "path"
            | "perf_hooks"
            | "process"
            | "querystring"
            | "readline"
            | "repl"
            | "stream"
            | "string_decoder"
            | "test"
            | "timers"
            | "tls"
            | "tty"
            | "url"
            | "util"
            | "v8"
            | "vm"
            | "wasi"
            | "worker_threads"
            | "zlib"
    )
}
