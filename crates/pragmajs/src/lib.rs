//! Combined check: one `pragma_parse` result, then own and rt on that program.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pragma_own::{CheckResult, Runtime};
use pragma_parse::{parse, Allocator, Parsed};
use pragma_rt::prelude::Environment;
use pragma_rt::syntax::{Annotation, RtError};
use pragma_rt::type_provider::{CompilerTypeProviderError, CorsaTypeProvider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerOptions {
    pub corsa_path: String,
    pub tsconfig_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    pub runtime: Runtime,
    pub environment: Environment,
    pub compiler: Option<CompilerOptions>,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            runtime: Runtime::default(),
            environment: Environment::Auto,
            compiler: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Check {
        paths: Vec<PathBuf>,
        options: CheckOptions,
    },
    Build {
        input_path: String,
        output_path: String,
        options: CheckOptions,
    },
}

#[derive(Debug)]
pub enum CombinedError {
    Io(io::Error),
    Compiler(CompilerTypeProviderError),
    Path(String),
}

impl std::fmt::Display for CombinedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Compiler(error) => write!(f, "Compiler type analysis failed: {error}"),
            Self::Path(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CombinedError {}

impl From<io::Error> for CombinedError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone)]
pub struct CombinedCheck {
    pub filename: String,
    pub parse_diagnostics: Vec<String>,
    pub own: CheckResult,
    pub rt: Vec<RtError>,
    pub rt_annotations: Vec<Annotation>,
}

impl CombinedCheck {
    pub fn failed(&self) -> bool {
        !self.parse_diagnostics.is_empty() || self.own.failed() || !self.rt.is_empty()
    }

    pub fn formatted_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for diagnostic in &self.parse_diagnostics {
            lines.push(format!("{}: error: {diagnostic}", self.filename));
        }
        lines.extend(self.own.formatted_lines());
        for error in &self.rt {
            let loc = error
                .loc
                .as_ref()
                .map(|location| {
                    format!(
                        "{}:{}:{}: ",
                        location.file.as_deref().unwrap_or(""),
                        location.line,
                        location.column
                    )
                })
                .unwrap_or_default();
            lines.push(format!("{}{}", loc, error.message));
        }
        lines
    }
}

/// Run own and rt on a program already produced by `pragma_parse`.
///
/// Does not parse the source again.
pub fn check_parsed(
    filename: &str,
    source: &str,
    parsed: &Parsed<'_>,
    options: &CheckOptions,
) -> Result<CombinedCheck, CombinedError> {
    let own = pragma_own::check_parsed_with(filename, source, &parsed.program, options.runtime);

    let (rt_annotations, mut rt) =
        match pragma_rt::parser::annotations_from_program(source, filename, &parsed.program) {
            Ok(result) => (result.annotations, Vec::new()),
            Err(message) => (Vec::new(), vec![RtError { message, loc: None }]),
        };

    if rt.is_empty() {
        rt = if let Some(compiler) = &options.compiler {
            let resolved = resolve_compiler(filename, compiler)?;
            let provider = CorsaTypeProvider::new(resolved.corsa_path, resolved.working_directory);
            pragma_rt::checker::check_program_with_environment_and_compiler(
                source,
                filename,
                &parsed.program,
                &rt_annotations,
                options.environment,
                &provider,
                &resolved.tsconfig_path,
                &resolved.source_path,
            )
            .map_err(CombinedError::Compiler)?
        } else {
            pragma_rt::checker::check_program_with_environment(
                source,
                filename,
                &parsed.program,
                &rt_annotations,
                options.environment,
            )
        };
    }

    Ok(CombinedCheck {
        filename: filename.to_string(),
        parse_diagnostics: parsed.diagnostics.clone(),
        own,
        rt,
        rt_annotations,
    })
}

/// Parse once, then [`check_parsed`].
pub fn check_source(
    filename: &str,
    source: &str,
    options: &CheckOptions,
) -> Result<CombinedCheck, CombinedError> {
    let allocator = Allocator::new();
    let parsed = parse(&allocator, filename, source);
    check_parsed(filename, source, &parsed, options)
}

pub struct EmitResult {
    pub check: CombinedCheck,
    pub code: Option<String>,
}

/// Parse once, check own and rt on that program, then emit `__rt.assert` JavaScript
/// from the same program.
pub fn emit_source(
    filename: &str,
    source: &str,
    options: &CheckOptions,
) -> Result<EmitResult, CombinedError> {
    let allocator = Allocator::new();
    let mut parsed = parse(&allocator, filename, source);
    let check = check_parsed(filename, source, &parsed, options)?;
    if check.failed() {
        return Ok(EmitResult { check, code: None });
    }
    let transformed = pragma_rt::transpiler::transpile_program(
        &allocator,
        &mut parsed.program,
        &check.rt_annotations,
    );
    let code = format!("{}\n\n{}", pragma_rt::runtime::runtime_block(), transformed);
    Ok(EmitResult {
        check,
        code: Some(code),
    })
}

struct ResolvedCompiler {
    corsa_path: PathBuf,
    tsconfig_path: PathBuf,
    working_directory: PathBuf,
    source_path: PathBuf,
}

fn canonicalize_path(path: &str, role: &str) -> Result<PathBuf, CombinedError> {
    fs::canonicalize(path)
        .map_err(|error| CombinedError::Path(format!("Failed to resolve {role} '{path}': {error}")))
}

fn resolve_compiler(
    filename: &str,
    compiler: &CompilerOptions,
) -> Result<ResolvedCompiler, CombinedError> {
    let source_path = canonicalize_path(filename, "source file")?;
    let corsa_path = canonicalize_path(&compiler.corsa_path, "Corsa executable")?;
    let tsconfig_path = canonicalize_path(&compiler.tsconfig_path, "TypeScript config")?;
    let working_directory = tsconfig_path
        .parent()
        .ok_or_else(|| {
            CombinedError::Path(format!(
                "TypeScript config '{}' has no parent directory",
                tsconfig_path.display()
            ))
        })?
        .to_path_buf();
    Ok(ResolvedCompiler {
        corsa_path,
        tsconfig_path,
        working_directory,
        source_path,
    })
}

const TARGET_VALUES: &str = "auto, ecmascript, browser, node, deno, bun";
const RUNTIME_VALUES: &str = "node, bun, deno, none";

pub fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("Usage: pragmajs <check|build> [args]".to_string());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Command::Help);
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        return Ok(Command::Version);
    }

    let (command, rest) = args.split_first().expect("args is non-empty");
    match command.as_str() {
        "check" => {
            let (options, positional) = parse_options(rest)?;
            if positional.is_empty() {
                return Err(format!(
                    "Usage: pragmajs check [--runtime <{RUNTIME_VALUES}>] [--target <{TARGET_VALUES}>] [--corsa <executable> --tsconfig <file>] <file-or-dir>..."
                ));
            }
            Ok(Command::Check {
                paths: positional.into_iter().map(PathBuf::from).collect(),
                options,
            })
        }
        "build" => {
            let (options, positional) = parse_options(rest)?;
            if positional.len() != 2 {
                return Err(format!(
                    "Usage: pragmajs build [--runtime <{RUNTIME_VALUES}>] [--target <{TARGET_VALUES}>] [--corsa <executable> --tsconfig <file>] <input> <output>"
                ));
            }
            Ok(Command::Build {
                input_path: positional[0].clone(),
                output_path: positional[1].clone(),
                options,
            })
        }
        _ => Err("Unknown command. Use 'check' or 'build'.".to_string()),
    }
}

fn parse_options(args: &[String]) -> Result<(CheckOptions, Vec<String>), String> {
    let mut runtime = Runtime::default();
    let mut environment = Environment::Auto;
    let mut target_seen = false;
    let mut runtime_seen = false;
    let mut corsa_path = None;
    let mut tsconfig_path = None;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let argument = &args[index];

        let runtime_value = if argument == "--runtime" || argument == "-r" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| {
                        format!("Missing value for '--runtime'. Expected one of: {RUNTIME_VALUES}.")
                    })?
                    .as_str(),
            )
        } else {
            argument.strip_prefix("--runtime=")
        };
        if let Some(value) = runtime_value {
            if runtime_seen {
                return Err("Runtime specified more than once.".to_string());
            }
            runtime = Runtime::parse(value).ok_or_else(|| {
                format!("Unknown runtime '{value}'. Expected one of: {RUNTIME_VALUES}.")
            })?;
            runtime_seen = true;
            index += 1;
            continue;
        }

        let target = if argument == "--target" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| {
                        format!("Missing value for '--target'. Expected one of: {TARGET_VALUES}.")
                    })?
                    .as_str(),
            )
        } else {
            argument.strip_prefix("--target=")
        };
        if let Some(target) = target {
            if target_seen {
                return Err("Target specified more than once.".to_string());
            }
            environment = parse_target(target)?;
            target_seen = true;
            index += 1;
            continue;
        }

        if argument == "--corsa" || argument.starts_with("--corsa=") {
            if corsa_path.is_some() {
                return Err("Corsa executable specified more than once.".to_string());
            }
            let value = if argument == "--corsa" {
                index += 1;
                args.get(index)
                    .ok_or_else(|| "Missing value for '--corsa'.".to_string())?
                    .as_str()
            } else {
                argument.trim_start_matches("--corsa=")
            };
            if value.is_empty() {
                return Err("Missing value for '--corsa'.".to_string());
            }
            corsa_path = Some(value.to_string());
            index += 1;
            continue;
        }

        if argument == "--tsconfig" || argument.starts_with("--tsconfig=") {
            if tsconfig_path.is_some() {
                return Err("TypeScript config specified more than once.".to_string());
            }
            let value = if argument == "--tsconfig" {
                index += 1;
                args.get(index)
                    .ok_or_else(|| "Missing value for '--tsconfig'.".to_string())?
                    .as_str()
            } else {
                argument.trim_start_matches("--tsconfig=")
            };
            if value.is_empty() {
                return Err("Missing value for '--tsconfig'.".to_string());
            }
            tsconfig_path = Some(value.to_string());
            index += 1;
            continue;
        }

        if argument.starts_with('-') {
            return Err(format!("Unknown option '{argument}'."));
        }
        positional.push(argument.clone());
        index += 1;
    }

    let compiler = match (corsa_path, tsconfig_path) {
        (Some(corsa_path), Some(tsconfig_path)) => Some(CompilerOptions {
            corsa_path,
            tsconfig_path,
        }),
        (Some(_), None) => return Err("'--corsa' requires '--tsconfig'.".to_string()),
        (None, Some(_)) => return Err("'--tsconfig' requires '--corsa'.".to_string()),
        (None, None) => None,
    };

    Ok((
        CheckOptions {
            runtime,
            environment,
            compiler,
        },
        positional,
    ))
}

fn parse_target(value: &str) -> Result<Environment, String> {
    match value {
        "auto" => Ok(Environment::Auto),
        "ecmascript" => Ok(Environment::Ecmascript),
        "browser" => Ok(Environment::Browser),
        "node" => Ok(Environment::Node),
        "deno" => Ok(Environment::Deno),
        "bun" => Ok(Environment::Bun),
        _ => Err(format!(
            "Unknown target '{value}'. Expected one of: {TARGET_VALUES}."
        )),
    }
}

pub fn collect_js_ts_files(path: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let child = entry.path();
            if child.is_dir() {
                if let Some(name) = child.file_name().and_then(|s| s.to_str()) {
                    if name == "node_modules" || name == "target" || name.starts_with('.') {
                        continue;
                    }
                }
                collect_js_ts_files(&child, out)?;
            } else if is_js_ts(&child) {
                out.push(child);
            }
        }
    } else if is_js_ts(path) || path.exists() {
        out.push(path.to_path_buf());
    }
    Ok(())
}

fn is_js_ts(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx")
    )
}

pub fn help_text() -> &'static str {
    "pragmajs — parse once, then run /*#own and /*#rt checks\n\n\
     Usage:\n\
         pragmajs check [--runtime node|bun|deno|none] [--target auto|ecmascript|browser|node|deno|bun]\n\
                 [--corsa <executable> --tsconfig <file>] <file-or-dir>...\n\
         pragmajs build [--runtime node|bun|deno|none] [--target auto|ecmascript|browser|node|deno|bun]\n\
                 [--corsa <executable> --tsconfig <file>] <input> <output>\n\n\
     --runtime, -r   Ownership prelude: node (default), bun, deno, or none\n\
     --target        Refinement prelude: auto (default), ecmascript, browser, node, deno, bun\n\
     --corsa         Corsa executable (requires --tsconfig)\n\
     --tsconfig      TypeScript config (requires --corsa)\n\n\
     check reports ownership and refinement diagnostics. build also writes JavaScript\n\
     that preserves __rt.assert after both checks succeed."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn positional_check_defaults() {
        assert_eq!(
            parse_args(&args(&["check", "input.js"])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions::default(),
            })
        );
    }

    #[test]
    fn positional_build_defaults() {
        assert_eq!(
            parse_args(&args(&["build", "input.js", "output.js"])),
            Ok(Command::Build {
                input_path: "input.js".to_string(),
                output_path: "output.js".to_string(),
                options: CheckOptions::default(),
            })
        );
    }

    #[test]
    fn runtime_and_target_can_precede_or_follow() {
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--runtime",
                "bun",
                "--target",
                "node",
                "input.js"
            ])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    runtime: Runtime::Bun,
                    environment: Environment::Node,
                    compiler: None,
                },
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "build",
                "input.js",
                "output.js",
                "--target=bun",
                "--runtime=none"
            ])),
            Ok(Command::Build {
                input_path: "input.js".to_string(),
                output_path: "output.js".to_string(),
                options: CheckOptions {
                    runtime: Runtime::None,
                    environment: Environment::Bun,
                    compiler: None,
                },
            })
        );
    }

    #[test]
    fn compiler_options_must_be_supplied_as_a_pair() {
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--corsa",
                "/tools/corsa",
                "--tsconfig=/project/tsconfig.json",
                "input.js",
            ])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: Some(CompilerOptions {
                        corsa_path: "/tools/corsa".to_string(),
                        tsconfig_path: "/project/tsconfig.json".to_string(),
                    }),
                },
            })
        );
        assert_eq!(
            parse_args(&args(&["check", "--corsa", "/tools/corsa", "input.js"])),
            Err("'--corsa' requires '--tsconfig'.".to_string())
        );
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--tsconfig",
                "/project/tsconfig.json",
                "input.js",
            ])),
            Err("'--tsconfig' requires '--corsa'.".to_string())
        );
    }

    #[test]
    fn unknown_target_has_deterministic_diagnostic() {
        assert_eq!(
            parse_args(&args(&["check", "--target", "workerd", "input.js"])),
            Err(
                "Unknown target 'workerd'. Expected one of: auto, ecmascript, browser, node, deno, bun."
                    .to_string()
            )
        );
    }
}
