//! Static ownership and borrow checker for JavaScript/TypeScript.
//!
//! Reads `/*#own ... */` comments, parses JS/TS with oxc, and reports
//! use-after-move, double-move, borrow conflicts, and lifetime errors.
//! No runtime code is generated or injected.

mod annot;
mod check;
mod prelude;

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

pub use annot::{BorrowMode, FnSig, OwnDirective, OwnType};
pub use check::{
    check_program, check_program_with_features, check_program_with_payloads,
    check_program_with_payloads_and_features, omitted_payload_offsets, own_payload_name,
    PayloadNames,
};
pub use pragma_loc::offset_to_line_col;
pub use prelude::Runtime;

/// One semantic or engineering assumption that can be removed in an ablation run.
///
/// These switches alter the checker path itself. They are intentionally separate
/// from filtering diagnostics after checking, which would leave the state machine
/// intact and produce misleading experiments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnAblation {
    FunctionContracts,
    MoveTracking,
    ExactOnce,
    AffineKind,
    BorrowModel,
    LocalBorrowDirectives,
    LocalCloneDirectives,
    LocalDropDirectives,
    LocalKindDirectives,
    LocalCalleeContracts,
    OwnedReturnPropagation,
    InstanceDispatch,
    ControlFlowSplitting,
    LoopDepth,
    NonConsumingPaths,
    UnknownCallConservatism,
    OptionalCallPaths,
    UnmappedGuards,
}

impl OwnAblation {
    pub const ALL: [Self; 18] = [
        Self::FunctionContracts,
        Self::MoveTracking,
        Self::ExactOnce,
        Self::AffineKind,
        Self::BorrowModel,
        Self::LocalBorrowDirectives,
        Self::LocalCloneDirectives,
        Self::LocalDropDirectives,
        Self::LocalKindDirectives,
        Self::LocalCalleeContracts,
        Self::OwnedReturnPropagation,
        Self::InstanceDispatch,
        Self::ControlFlowSplitting,
        Self::LoopDepth,
        Self::NonConsumingPaths,
        Self::UnknownCallConservatism,
        Self::OptionalCallPaths,
        Self::UnmappedGuards,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            Self::FunctionContracts => "function-contracts",
            Self::MoveTracking => "move-tracking",
            Self::ExactOnce => "exact-once",
            Self::AffineKind => "affine-kind",
            Self::BorrowModel => "borrow-model",
            Self::LocalBorrowDirectives => "local-borrow-directives",
            Self::LocalCloneDirectives => "local-clone-directives",
            Self::LocalDropDirectives => "local-drop-directives",
            Self::LocalKindDirectives => "local-kind-directives",
            Self::LocalCalleeContracts => "local-callee-contracts",
            Self::OwnedReturnPropagation => "owned-return-propagation",
            Self::InstanceDispatch => "instance-dispatch",
            Self::ControlFlowSplitting => "control-flow-splitting",
            Self::LoopDepth => "loop-depth",
            Self::NonConsumingPaths => "non-consuming-paths",
            Self::UnknownCallConservatism => "unknown-call-conservatism",
            Self::OptionalCallPaths => "optional-call-paths",
            Self::UnmappedGuards => "unmapped-guards",
        }
    }
}

/// Semantic features used by the ownership checker.
///
/// Production entry points use [`OwnFeatures::default`] (everything enabled).
/// [`OwnFeatures::without`] creates a one-factor-at-a-time experimental variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnFeatures {
    pub function_contracts: bool,
    pub move_tracking: bool,
    pub exact_once: bool,
    pub affine_kind: bool,
    pub borrow_model: bool,
    /// Named `borrow` and argument-position `&readonly` / `&mut` directives.
    pub local_borrow_directives: bool,
    /// Explicit `clone owner as alias` directives.
    pub local_clone_directives: bool,
    /// Explicit `drop name` directives.
    pub local_drop_directives: bool,
    /// Both `let name: kind` and binding-position kind shorthand directives.
    pub local_kind_directives: bool,
    pub local_callee_contracts: bool,
    pub owned_return_propagation: bool,
    pub instance_dispatch: bool,
    pub control_flow_splitting: bool,
    pub loop_depth: bool,
    pub non_consuming_paths: bool,
    pub unknown_call_conservatism: bool,
    pub optional_call_paths: bool,
    pub unmapped_guards: bool,
}

impl OwnFeatures {
    pub const fn all() -> Self {
        Self {
            function_contracts: true,
            move_tracking: true,
            exact_once: true,
            affine_kind: true,
            borrow_model: true,
            local_borrow_directives: true,
            local_clone_directives: true,
            local_drop_directives: true,
            local_kind_directives: true,
            local_callee_contracts: true,
            owned_return_propagation: true,
            instance_dispatch: true,
            control_flow_splitting: true,
            loop_depth: true,
            non_consuming_paths: true,
            unknown_call_conservatism: true,
            optional_call_paths: true,
            unmapped_guards: true,
        }
    }

    pub const fn without(ablation: OwnAblation) -> Self {
        let mut features = Self::all();
        match ablation {
            OwnAblation::FunctionContracts => features.function_contracts = false,
            OwnAblation::MoveTracking => features.move_tracking = false,
            OwnAblation::ExactOnce => features.exact_once = false,
            OwnAblation::AffineKind => features.affine_kind = false,
            OwnAblation::BorrowModel => features.borrow_model = false,
            OwnAblation::LocalBorrowDirectives => features.local_borrow_directives = false,
            OwnAblation::LocalCloneDirectives => features.local_clone_directives = false,
            OwnAblation::LocalDropDirectives => features.local_drop_directives = false,
            OwnAblation::LocalKindDirectives => features.local_kind_directives = false,
            OwnAblation::LocalCalleeContracts => features.local_callee_contracts = false,
            OwnAblation::OwnedReturnPropagation => features.owned_return_propagation = false,
            OwnAblation::InstanceDispatch => features.instance_dispatch = false,
            OwnAblation::ControlFlowSplitting => features.control_flow_splitting = false,
            OwnAblation::LoopDepth => features.loop_depth = false,
            OwnAblation::NonConsumingPaths => features.non_consuming_paths = false,
            OwnAblation::UnknownCallConservatism => features.unknown_call_conservatism = false,
            OwnAblation::OptionalCallPaths => features.optional_call_paths = false,
            OwnAblation::UnmappedGuards => features.unmapped_guards = false,
        }
        features
    }

}

impl Default for OwnFeatures {
    fn default() -> Self {
        Self::all()
    }
}

/// Kind of ownership/borrow rule that was violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleKind {
    UniqueForget,
    DoubleMove,
    UseAfterMove,
    ConsumeInLoop,
    BranchInconsistent,
    BorrowAfterMove,
    ConsumeWhileBorrowed,
    MutBorrowConflict,
    BorrowEscape,
    UnmappedConstruct,
    AnnotParseError,
    MissingType,
}

impl RuleKind {
    pub fn slug(self) -> &'static str {
        match self {
            RuleKind::UniqueForget => "unique-forget",
            RuleKind::DoubleMove => "double-move",
            RuleKind::UseAfterMove => "use-after-move",
            RuleKind::ConsumeInLoop => "consume-in-loop",
            RuleKind::BranchInconsistent => "branch-inconsistent",
            RuleKind::BorrowAfterMove => "borrow-after-move",
            RuleKind::ConsumeWhileBorrowed => "consume-while-borrowed",
            RuleKind::MutBorrowConflict => "mut-borrow-conflict",
            RuleKind::BorrowEscape => "borrow-escape",
            RuleKind::UnmappedConstruct => "unmapped",
            RuleKind::AnnotParseError => "annot-parse",
            RuleKind::MissingType => "missing-type",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: String,
    pub offset: u32,
    pub kind: RuleKind,
    pub message: String,
}

impl Diagnostic {
    pub fn line_col(&self, source: &str) -> (u32, u32) {
        offset_to_line_col(source, self.offset)
    }

    pub fn format_line(&self, source: &str) -> String {
        let (line, col) = self.line_col(source);
        format!(
            "{}:{}:{}: error[{}]: {}",
            self.path,
            line,
            col,
            self.kind.slug(),
            self.message
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    /// Original sources keyed by path, used to format diagnostics.
    pub sources: Vec<(String, String)>,
}

impl CheckResult {
    pub fn failed(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    pub fn kinds(&self) -> Vec<RuleKind> {
        self.diagnostics.iter().map(|d| d.kind).collect()
    }

    pub fn formatted_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for d in &self.diagnostics {
            let source = self
                .sources
                .iter()
                .find(|(p, _)| p == &d.path)
                .map(|(_, s)| s.as_str())
                .unwrap_or("");
            lines.push(d.format_line(source));
        }
        lines
    }

    fn merge(&mut self, other: CheckResult) {
        self.diagnostics.extend(other.diagnostics);
        self.sources.extend(other.sources);
    }
}

/// Check a single source string. `filename` selects JS vs TS via extension.
/// Loads the Node builtin prelude (same as [`Runtime::Node`]).
pub fn check_source(filename: &str, source: &str) -> CheckResult {
    check_source_with(filename, source, Runtime::default())
}

/// Like [`check_source`], with an explicit runtime prelude.
pub fn check_source_with(filename: &str, source: &str, runtime: Runtime) -> CheckResult {
    check_source_with_features(filename, source, runtime, OwnFeatures::default())
}

/// Check a source string with an explicit semantic feature set.
///
/// This is an experimental entry point for ablation studies; normal callers
/// should use [`check_source_with`].
pub fn check_source_with_features(
    filename: &str,
    source: &str,
    runtime: Runtime,
    features: OwnFeatures,
) -> CheckResult {
    let diagnostics = check::check_source_with_features(filename, source, runtime, features);
    CheckResult {
        diagnostics,
        sources: vec![(filename.to_string(), source.to_string())],
    }
}

/// Like [`check_source_with`], using a program already produced by `pragma_parse`.
pub fn check_parsed_with(
    filename: &str,
    source: &str,
    program: &pragma_parse::Program<'_>,
    runtime: Runtime,
) -> CheckResult {
    check_parsed_with_features(filename, source, program, runtime, OwnFeatures::default())
}

/// Like [`check_parsed_with`], with an explicit semantic feature set.
pub fn check_parsed_with_features(
    filename: &str,
    source: &str,
    program: &pragma_parse::Program<'_>,
    runtime: Runtime,
    features: OwnFeatures,
) -> CheckResult {
    check_parsed_with_payloads_and_features(filename, source, program, runtime, None, features)
}

/// Like [`check_parsed_with`], filling omitted payload names from `payloads`.
pub fn check_parsed_with_payloads(
    filename: &str,
    source: &str,
    program: &pragma_parse::Program<'_>,
    runtime: Runtime,
    payloads: Option<&dyn PayloadNames>,
) -> CheckResult {
    check_parsed_with_payloads_and_features(
        filename,
        source,
        program,
        runtime,
        payloads,
        OwnFeatures::default(),
    )
}

/// Like [`check_parsed_with_payloads`], with an explicit semantic feature set.
pub fn check_parsed_with_payloads_and_features(
    filename: &str,
    source: &str,
    program: &pragma_parse::Program<'_>,
    runtime: Runtime,
    payloads: Option<&dyn PayloadNames>,
    features: OwnFeatures,
) -> CheckResult {
    CheckResult {
        diagnostics: check_program_with_payloads_and_features(
            filename, source, program, runtime, payloads, features,
        ),
        sources: vec![(filename.to_string(), source.to_string())],
    }
}

/// Check a file on disk with an explicit runtime prelude.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_path_with(path: &Path, runtime: Runtime) -> io::Result<CheckResult> {
    let source = fs::read_to_string(path)?;
    let name = path.to_string_lossy().to_string();
    Ok(check_source_with(&name, &source, runtime))
}

/// Check files and/or directories (recursively, `.js`/`.ts`/`.mjs`/`.cjs`/`.jsx`/`.tsx`).
#[cfg(not(target_arch = "wasm32"))]
pub fn check_paths(paths: &[PathBuf]) -> io::Result<CheckResult> {
    check_paths_with(paths, Runtime::default())
}

/// Like [`check_paths`], with an explicit runtime prelude.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_paths_with(paths: &[PathBuf], runtime: Runtime) -> io::Result<CheckResult> {
    let mut files = Vec::new();
    for p in paths {
        collect_js_ts_files(p, &mut files)?;
    }
    files.sort();
    let mut acc = CheckResult::default();
    for f in files {
        acc.merge(check_path_with(&f, runtime)?);
    }
    Ok(acc)
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_js_ts_files(path: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name == "node_modules" || name == "target" || name.starts_with('.') {
                        continue;
                    }
                }
                collect_js_ts_files(&p, out)?;
            } else if is_js_ts(&p) {
                out.push(p);
            }
        }
    } else if is_js_ts(path) || path.exists() {
        out.push(path.to_path_buf());
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn is_js_ts(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx")
    )
}
