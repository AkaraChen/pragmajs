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
pub use prelude::Runtime;

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
    let diagnostics = check::check_source(filename, source, runtime);
    CheckResult {
        diagnostics,
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

pub fn offset_to_line_col(source: &str, offset: u32) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in source.char_indices() {
        if i as u32 >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_mapping() {
        assert_eq!(offset_to_line_col("ab\ncd", 0), (1, 1));
        assert_eq!(offset_to_line_col("ab\ncd", 3), (2, 1));
    }
}
