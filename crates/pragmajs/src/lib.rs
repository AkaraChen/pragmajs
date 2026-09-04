//! Combined check: one `pragma_parse` result, then own and rt on that program.

mod bundle;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pragma_own::{
    check_parsed_with, check_parsed_with_payloads, omitted_payload_offsets, own_payload_name,
    CheckResult, Runtime,
};
use pragma_parse::{parse, Allocator, ParseDiagnostic, Parsed};
use pragma_rt::prelude::Environment;
use pragma_rt::syntax::{Annotation, RtError};
use pragma_rt::type_provider::{
    CompilerDiagnostic, CompilerDiagnosticSeverity, CompilerTypeProvider,
    CompilerTypeProviderError, CorsaTypeProvider,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerOptions {
    pub corsa_path: Option<String>,
    pub tsconfig_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilerMode {
    /// Discover Corsa and `tsconfig.json` from the source file. Skip if missing.
    Auto,
    Off,
    On(CompilerOptions),
}

/// Select which checker participates in a run.
///
/// `All` preserves the original combined-check behavior. The single-checker
/// variants are useful as an ablation axis and do not invoke the other
/// checker, including its annotation parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckerSelection {
    All,
    Own,
    Rt,
}

impl CheckerSelection {
    const fn includes_own(self) -> bool {
        matches!(self, Self::All | Self::Own)
    }

    const fn includes_rt(self) -> bool {
        matches!(self, Self::All | Self::Rt)
    }
}

impl Default for CheckerSelection {
    fn default() -> Self {
        Self::All
    }
}

/// A coherent platform choice shared by the ownership and refinement checkers.
///
/// Unlike the low-level `runtime` and `environment` settings, a profile cannot
/// select different platforms for the two checkers. There is deliberately no
/// `Auto` profile: refinement auto-detection has no ownership-runtime
/// equivalent, so exposing it here would not be an atomic platform choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformProfile {
    Ecmascript,
    Browser,
    Node,
    Deno,
    Bun,
}

impl PlatformProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ecmascript" => Some(Self::Ecmascript),
            "browser" => Some(Self::Browser),
            "node" => Some(Self::Node),
            "deno" => Some(Self::Deno),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    /// Resolve both checker-specific settings as one indivisible choice.
    pub const fn settings(self) -> (Runtime, Environment) {
        match self {
            Self::Ecmascript => (Runtime::None, Environment::Ecmascript),
            Self::Browser => (Runtime::None, Environment::Browser),
            Self::Node => (Runtime::Node, Environment::Node),
            Self::Deno => (Runtime::Deno, Environment::Deno),
            Self::Bun => (Runtime::Bun, Environment::Bun),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ecmascript => "ecmascript",
            Self::Browser => "browser",
            Self::Node => "node",
            Self::Deno => "deno",
            Self::Bun => "bun",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    pub checker: CheckerSelection,
    pub runtime: Runtime,
    pub environment: Environment,
    pub compiler: CompilerMode,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            checker: CheckerSelection::All,
            runtime: Runtime::default(),
            environment: Environment::Auto,
            compiler: CompilerMode::Auto,
        }
    }
}

impl CheckOptions {
    /// Apply one coherent platform profile to both checkers.
    pub const fn with_platform(mut self, profile: PlatformProfile) -> Self {
        let (runtime, environment) = profile.settings();
        self.runtime = runtime;
        self.environment = environment;
        self
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
    pub parse_diagnostics: Vec<ParseDiagnostic>,
    pub own: CheckResult,
    pub rt: Vec<RtError>,
    pub rt_annotations: Vec<Annotation>,
    /// Raw diagnostics returned by the compiler analysis used for this check.
    ///
    /// When RT is selected, compiler errors also remain represented in `rt` and
    /// keep the existing CLI output and failure behavior. In own-only mode they
    /// remain observational and do not change [`Self::failed`]. This field lets
    /// experiments attribute them to the compiler instead of treating the
    /// combined exit status as a checker result.
    pub compiler_diagnostics: Vec<CompilerDiagnostic>,
}

impl CombinedCheck {
    pub fn failed(&self) -> bool {
        !self.parse_diagnostics.is_empty() || self.own.failed() || !self.rt.is_empty()
    }

    pub fn formatted_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for diagnostic in &self.parse_diagnostics {
            lines.push(format!("{}: error: {diagnostic:?}", self.filename));
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

    /// Refinement-checker diagnostics with compiler errors excluded.
    ///
    /// The refinement checker stops before verification when the compiler
    /// reports an error, so in that case every entry in `rt` is the formatted
    /// compiler error rather than a verifier finding.
    pub fn refinement_diagnostics(&self) -> &[RtError] {
        if self
            .compiler_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == CompilerDiagnosticSeverity::Error)
        {
            &[]
        } else {
            &self.rt
        }
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
    check_parsed_using(filename, source, parsed, options, CompilerInput::Discover)
}

/// Run the combined pipeline with an explicitly supplied compiler provider.
///
/// This is the deterministic counterpart to [`check_parsed`] for experiments
/// and provider integration tests: `options.compiler` still controls whether
/// compiler analysis is off, automatic, or explicitly forced, but executable
/// discovery is replaced by `provider`. The caller supplies the exact project
/// and source paths that are forwarded to the provider.
pub fn check_parsed_with_compiler_provider(
    filename: &str,
    source: &str,
    parsed: &Parsed<'_>,
    options: &CheckOptions,
    provider: &dyn CompilerTypeProvider,
    tsconfig_path: &Path,
    source_path: &Path,
) -> Result<CombinedCheck, CombinedError> {
    check_parsed_using(
        filename,
        source,
        parsed,
        options,
        CompilerInput::Provided {
            provider,
            tsconfig_path,
            source_path,
        },
    )
}

enum CompilerInput<'a> {
    Discover,
    Provided {
        provider: &'a dyn CompilerTypeProvider,
        tsconfig_path: &'a Path,
        source_path: &'a Path,
    },
}

fn check_parsed_using(
    filename: &str,
    source: &str,
    parsed: &Parsed<'_>,
    options: &CheckOptions,
    compiler_input: CompilerInput<'_>,
) -> Result<CombinedCheck, CombinedError> {
    let (rt_annotations, rt) = if options.checker.includes_rt() {
        match pragma_rt::parser::annotations_from_program(source, filename, &parsed.program) {
            Ok(result) => (result.annotations, Vec::new()),
            Err(message) => (Vec::new(), vec![RtError { message, loc: None }]),
        }
    } else {
        (Vec::new(), Vec::new())
    };
    let own_offsets = if options.checker.includes_own() {
        omitted_payload_offsets(filename, source, &parsed.program)
    } else {
        Vec::new()
    };
    let compiler_required =
        should_resolve_compiler(options.checker, &own_offsets, &options.compiler);

    if !rt.is_empty() || !compiler_required || matches!(options.compiler, CompilerMode::Off) {
        return Ok(check_without_compiler(
            filename,
            source,
            parsed,
            options,
            rt_annotations,
            rt,
        ));
    }

    match compiler_input {
        CompilerInput::Discover => {
            let Some(resolved) = compiler_for_file(filename, &options.compiler)? else {
                return Ok(check_without_compiler(
                    filename,
                    source,
                    parsed,
                    options,
                    rt_annotations,
                    rt,
                ));
            };
            let provider = CorsaTypeProvider::new(resolved.corsa_path, resolved.working_directory);
            check_with_compiler(
                filename,
                source,
                parsed,
                options,
                rt_annotations,
                rt,
                own_offsets,
                &provider,
                &resolved.tsconfig_path,
                &resolved.source_path,
            )
        }
        CompilerInput::Provided {
            provider,
            tsconfig_path,
            source_path,
        } => check_with_compiler(
            filename,
            source,
            parsed,
            options,
            rt_annotations,
            rt,
            own_offsets,
            provider,
            tsconfig_path,
            source_path,
        ),
    }
}

fn check_without_compiler(
    filename: &str,
    source: &str,
    parsed: &Parsed<'_>,
    options: &CheckOptions,
    rt_annotations: Vec<Annotation>,
    mut rt: Vec<RtError>,
) -> CombinedCheck {
    let own = if options.checker.includes_own() {
        check_parsed_with(filename, source, &parsed.program, options.runtime)
    } else {
        CheckResult::default()
    };
    if rt.is_empty() && options.checker.includes_rt() {
        rt = pragma_rt::checker::check_program_with_environment(
            source,
            filename,
            &parsed.program,
            &rt_annotations,
            options.environment,
        );
    }
    CombinedCheck {
        filename: filename.to_string(),
        parse_diagnostics: parsed.diagnostics.clone(),
        own,
        rt,
        rt_annotations,
        compiler_diagnostics: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn check_with_compiler(
    filename: &str,
    source: &str,
    parsed: &Parsed<'_>,
    options: &CheckOptions,
    rt_annotations: Vec<Annotation>,
    mut rt: Vec<RtError>,
    own_offsets: Vec<u32>,
    provider: &dyn CompilerTypeProvider,
    tsconfig_path: &Path,
    source_path: &Path,
) -> Result<CombinedCheck, CombinedError> {
    let rt_offsets = pragma_rt::parser::omitted_query_offsets(&rt_annotations);
    let extra = selected_compiler_offsets(options.checker, &own_offsets, &rt_offsets);
    let hints = pragma_rt::compiler_hints::analyze_program_with_offsets(
        provider,
        source,
        &parsed.program,
        tsconfig_path,
        source_path,
        &extra,
    )
    .map_err(CombinedError::Compiler)?;
    let compiler_diagnostics = hints.diagnostics().to_vec();
    let own = if options.checker.includes_own() {
        let mut payloads = HashMap::new();
        for offset in own_offsets {
            if let Some(rendered) = hints.rendered_at(offset as usize) {
                if let Some(name) = own_payload_name(rendered) {
                    payloads.insert(offset, name);
                }
            }
        }
        check_parsed_with_payloads(
            filename,
            source,
            &parsed.program,
            options.runtime,
            Some(&payloads),
        )
    } else {
        CheckResult::default()
    };
    if options.checker.includes_rt() {
        rt = pragma_rt::checker::check_program_with_hints(
            source,
            filename,
            &parsed.program,
            &rt_annotations,
            options.environment,
            source_path,
            &hints,
        );
    }
    Ok(CombinedCheck {
        filename: filename.to_string(),
        parse_diagnostics: parsed.diagnostics.clone(),
        own,
        rt,
        rt_annotations,
        compiler_diagnostics,
    })
}

fn selected_compiler_offsets(
    checker: CheckerSelection,
    own_offsets: &[u32],
    rt_offsets: &[usize],
) -> Vec<usize> {
    let mut offsets = Vec::new();
    if checker.includes_own() {
        offsets.extend(own_offsets.iter().map(|&offset| offset as usize));
    }
    if checker.includes_rt() {
        offsets.extend_from_slice(rt_offsets);
    }
    offsets.sort();
    offsets.dedup();
    offsets
}

fn should_resolve_compiler(
    checker: CheckerSelection,
    own_offsets: &[u32],
    compiler: &CompilerMode,
) -> bool {
    checker.includes_rt() || !own_offsets.is_empty() || matches!(compiler, CompilerMode::On(_))
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
    let code = if options.checker.includes_rt() {
        format!("{}\n\n{}", pragma_rt::runtime::runtime_block(), transformed)
    } else {
        transformed
    };
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

fn compiler_for_file(
    filename: &str,
    mode: &CompilerMode,
) -> Result<Option<ResolvedCompiler>, CombinedError> {
    match mode {
        CompilerMode::Off => Ok(None),
        CompilerMode::Auto => Ok(discover_compiler(filename, None, None, false)),
        CompilerMode::On(options) => {
            match discover_compiler(
                filename,
                options.corsa_path.as_deref(),
                options.tsconfig_path.as_deref(),
                true,
            ) {
                Some(resolved) => Ok(Some(resolved)),
                None => {
                    if options.corsa_path.is_none() && find_corsa_executable().is_none() {
                        Err(CombinedError::Path(
                            "Corsa executable not found; pass --corsa or set CORSA_BIN / TSGO"
                                .into(),
                        ))
                    } else {
                        Err(CombinedError::Path(
                            "TypeScript config not found; pass --tsconfig".into(),
                        ))
                    }
                }
            }
        }
    }
}

fn discover_compiler(
    filename: &str,
    corsa_override: Option<&str>,
    tsconfig_override: Option<&str>,
    required: bool,
) -> Option<ResolvedCompiler> {
    let source_path = fs::canonicalize(filename).ok()?;
    let corsa_path = match corsa_override {
        Some(path) => fs::canonicalize(path).ok(),
        None => find_corsa_executable().or_else(|| bundle::bundled_tsgo().ok()),
    }?;
    let tsconfig_path = match tsconfig_override {
        Some(path) => fs::canonicalize(path).ok(),
        None => find_tsconfig(&source_path).or_else(|| {
            if required || corsa_override.is_none() {
                synthesize_tsconfig(&source_path)
            } else {
                None
            }
        }),
    }?;
    let working_directory = tsconfig_path.parent()?.to_path_buf();
    Some(ResolvedCompiler {
        corsa_path,
        tsconfig_path,
        working_directory,
        source_path,
    })
}

fn find_corsa_executable() -> Option<PathBuf> {
    for key in ["CORSA_BIN", "TSGO"] {
        if let Ok(value) = env::var(key) {
            if !value.is_empty() {
                let path = PathBuf::from(value);
                if path.is_file() {
                    return fs::canonicalize(path).ok();
                }
            }
        }
    }
    if let Some(path) = bundle::cached_tsgo() {
        return Some(path);
    }
    for name in ["corsa", "tsgo"] {
        if let Some(path) = which(name) {
            return Some(path);
        }
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return fs::canonicalize(candidate).ok();
        }
    }
    None
}

fn find_tsconfig(source: &Path) -> Option<PathBuf> {
    let mut dir = if source.is_file() {
        source.parent()?.to_path_buf()
    } else if source.is_dir() {
        source.to_path_buf()
    } else {
        return None;
    };
    loop {
        let candidate = dir.join("tsconfig.json");
        if candidate.is_file() {
            return fs::canonicalize(candidate).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn synthesize_tsconfig(source: &Path) -> Option<PathBuf> {
    let source = fs::canonicalize(source).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&source, &mut hasher);
    let path = env::temp_dir().join(format!(
        "pragmajs-tsconfig-{:x}.json",
        std::hash::Hasher::finish(&hasher)
    ));
    let file = json_string(&source.to_string_lossy());
    let body = format!(
        "{{\n  \"compilerOptions\": {{\n    \"strict\": true,\n    \"strictNullChecks\": true,\n    \"checkJs\": true,\n    \"allowJs\": true,\n    \"noEmit\": true\n  }},\n  \"files\": [{file}]\n}}\n"
    );
    fs::write(&path, body).ok()?;
    fs::canonicalize(path).ok()
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const TARGET_VALUES: &str = "auto, ecmascript, browser, node, deno, bun";
const RUNTIME_VALUES: &str = "node, bun, deno, none";
const CHECKER_VALUES: &str = "all, own, rt";
const PLATFORM_VALUES: &str = "ecmascript, browser, node, deno, bun";

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
                    "Usage: pragmajs check [--checker <{CHECKER_VALUES}>] [--platform <{PLATFORM_VALUES}>] [--runtime <{RUNTIME_VALUES}>] [--target <{TARGET_VALUES}>] [--corsa <executable>] [--tsconfig <file>] [--no-corsa] <file-or-dir>..."
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
                    "Usage: pragmajs build [--checker <{CHECKER_VALUES}>] [--platform <{PLATFORM_VALUES}>] [--runtime <{RUNTIME_VALUES}>] [--target <{TARGET_VALUES}>] [--corsa <executable>] [--tsconfig <file>] [--no-corsa] <input> <output>"
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
    let mut checker = CheckerSelection::All;
    let mut runtime = Runtime::default();
    let mut environment = Environment::Auto;
    let mut platform = None;
    let mut platform_seen = false;
    let mut target_seen = false;
    let mut runtime_seen = false;
    let mut checker_seen = false;
    let mut corsa_path = None;
    let mut tsconfig_path = None;
    let mut corsa_off = false;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let argument = &args[index];

        let checker_value = if argument == "--checker" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| {
                        format!("Missing value for '--checker'. Expected one of: {CHECKER_VALUES}.")
                    })?
                    .as_str(),
            )
        } else {
            argument.strip_prefix("--checker=")
        };
        if let Some(value) = checker_value {
            if checker_seen {
                return Err("Checker specified more than once.".to_string());
            }
            checker = parse_checker(value)?;
            checker_seen = true;
            index += 1;
            continue;
        }

        let platform_value = if argument == "--platform" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| {
                        format!(
                            "Missing value for '--platform'. Expected one of: {PLATFORM_VALUES}."
                        )
                    })?
                    .as_str(),
            )
        } else {
            argument.strip_prefix("--platform=")
        };
        if let Some(value) = platform_value {
            if platform_seen {
                return Err("Platform specified more than once.".to_string());
            }
            platform = Some(PlatformProfile::parse(value).ok_or_else(|| {
                format!("Unknown platform '{value}'. Expected one of: {PLATFORM_VALUES}.")
            })?);
            platform_seen = true;
            index += 1;
            continue;
        }

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

        if argument == "--no-corsa" {
            if corsa_off {
                return Err("Corsa disabled more than once.".to_string());
            }
            corsa_off = true;
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

    if corsa_off && (corsa_path.is_some() || tsconfig_path.is_some()) {
        return Err("'--no-corsa' cannot be combined with '--corsa' or '--tsconfig'.".to_string());
    }
    if platform_seen && (runtime_seen || target_seen) {
        return Err(
            "'--platform' cannot be combined with the low-level '--runtime' or '--target' options."
                .to_string(),
        );
    }

    if let Some(profile) = platform {
        (runtime, environment) = profile.settings();
    }

    let compiler = if corsa_off {
        CompilerMode::Off
    } else if corsa_path.is_none() && tsconfig_path.is_none() {
        CompilerMode::Auto
    } else {
        CompilerMode::On(CompilerOptions {
            corsa_path,
            tsconfig_path,
        })
    };

    Ok((
        CheckOptions {
            checker,
            runtime,
            environment,
            compiler,
        },
        positional,
    ))
}

fn parse_checker(value: &str) -> Result<CheckerSelection, String> {
    match value {
        "all" => Ok(CheckerSelection::All),
        "own" => Ok(CheckerSelection::Own),
        "rt" => Ok(CheckerSelection::Rt),
        _ => Err(format!(
            "Unknown checker '{value}'. Expected one of: {CHECKER_VALUES}."
        )),
    }
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
         pragmajs check [--checker all|own|rt] [--platform ecmascript|browser|node|deno|bun]\n\
                 [--runtime node|bun|deno|none] [--target auto|ecmascript|browser|node|deno|bun]\n\
                 [--corsa <executable>] [--tsconfig <file>] [--no-corsa] <file-or-dir>...\n\
         pragmajs build [--checker all|own|rt] [--platform ecmascript|browser|node|deno|bun]\n\
                 [--runtime node|bun|deno|none] [--target auto|ecmascript|browser|node|deno|bun]\n\
                 [--corsa <executable>] [--tsconfig <file>] [--no-corsa] <input> <output>\n\n\
     --checker       Checker selection: all (default), own, or rt\n\
     --platform      Set both checker preludes: ecmascript, browser, node, deno, or bun\n\
     --runtime, -r   Low-level ownership override: node (default), bun, deno, or none\n\
     --target        Low-level refinement override: auto (default), ecmascript, browser, node, deno, bun\n\
     --corsa         Corsa executable (default: bundled tsgo, else CORSA_BIN / PATH)\n\
     --tsconfig      TypeScript config (default: nearest tsconfig.json, else a temp project)\n\
     --no-corsa      Do not query TypeScript\n\n\
     --platform cannot be combined with --runtime or --target. Corsa is on by default.\n\
     check reports ownership and refinement diagnostics.\n\
     build also writes JavaScript that preserves __rt.assert after both checks succeed."
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
    fn checker_selection_defaults_to_all_and_parses_explicit_values() {
        assert_eq!(
            parse_args(&args(&["check", "input.js"])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions::default(),
            })
        );
        for (value, checker) in [
            ("all", CheckerSelection::All),
            ("own", CheckerSelection::Own),
            ("rt", CheckerSelection::Rt),
        ] {
            assert_eq!(
                parse_args(&args(&["check", "--checker", value, "input.js"])),
                Ok(Command::Check {
                    paths: vec![PathBuf::from("input.js")],
                    options: CheckOptions {
                        checker,
                        ..CheckOptions::default()
                    },
                })
            );
            assert_eq!(
                parse_args(&args(&[
                    "build",
                    "input.js",
                    "output.js",
                    &format!("--checker={value}"),
                ])),
                Ok(Command::Build {
                    input_path: "input.js".to_string(),
                    output_path: "output.js".to_string(),
                    options: CheckOptions {
                        checker,
                        ..CheckOptions::default()
                    },
                })
            );
        }
    }

    #[test]
    fn checker_selection_rejects_missing_unknown_and_duplicate_values() {
        assert_eq!(
            parse_args(&args(&["check", "input.js", "--checker"])),
            Err("Missing value for '--checker'. Expected one of: all, own, rt.".to_string())
        );
        assert_eq!(
            parse_args(&args(&["check", "--checker", "both", "input.js"])),
            Err("Unknown checker 'both'. Expected one of: all, own, rt.".to_string())
        );
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--checker=own",
                "--checker",
                "rt",
                "input.js",
            ])),
            Err("Checker specified more than once.".to_string())
        );
    }

    #[test]
    fn platform_profiles_atomically_map_both_checkers() {
        for (value, profile, runtime, environment) in [
            (
                "ecmascript",
                PlatformProfile::Ecmascript,
                Runtime::None,
                Environment::Ecmascript,
            ),
            (
                "browser",
                PlatformProfile::Browser,
                Runtime::None,
                Environment::Browser,
            ),
            (
                "node",
                PlatformProfile::Node,
                Runtime::Node,
                Environment::Node,
            ),
            (
                "deno",
                PlatformProfile::Deno,
                Runtime::Deno,
                Environment::Deno,
            ),
            ("bun", PlatformProfile::Bun, Runtime::Bun, Environment::Bun),
        ] {
            assert_eq!(PlatformProfile::parse(value), Some(profile));
            assert_eq!(profile.as_str(), value);
            assert_eq!(profile.settings(), (runtime, environment));
            assert_eq!(
                CheckOptions::default().with_platform(profile),
                CheckOptions {
                    checker: CheckerSelection::All,
                    runtime,
                    environment,
                    compiler: CompilerMode::Auto,
                }
            );
            assert_eq!(
                parse_args(&args(&["check", "--platform", value, "input.js"])),
                Ok(Command::Check {
                    paths: vec![PathBuf::from("input.js")],
                    options: CheckOptions {
                        checker: CheckerSelection::All,
                        runtime,
                        environment,
                        compiler: CompilerMode::Auto,
                    },
                })
            );
        }

        assert_eq!(
            parse_args(&args(
                &["build", "input.js", "output.js", "--platform=bun",]
            )),
            Ok(Command::Build {
                input_path: "input.js".to_string(),
                output_path: "output.js".to_string(),
                options: CheckOptions::default().with_platform(PlatformProfile::Bun),
            })
        );
    }

    #[test]
    fn platform_rejects_non_atomic_or_duplicate_configuration() {
        assert_eq!(
            parse_args(&args(&["check", "input.js", "--platform"])),
            Err(
                "Missing value for '--platform'. Expected one of: ecmascript, browser, node, deno, bun."
                    .to_string()
            )
        );
        assert_eq!(
            parse_args(&args(&["check", "--platform", "auto", "input.js"])),
            Err(
                "Unknown platform 'auto'. Expected one of: ecmascript, browser, node, deno, bun."
                    .to_string()
            )
        );
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--platform=node",
                "--platform",
                "bun",
                "input.js",
            ])),
            Err("Platform specified more than once.".to_string())
        );
        for options in [
            ["--platform", "bun", "--runtime", "bun"],
            ["--target", "bun", "--platform", "bun"],
        ] {
            let mut values = vec!["check"];
            values.extend(options);
            values.push("input.js");
            assert_eq!(
                parse_args(&args(&values)),
                Err(
                    "'--platform' cannot be combined with the low-level '--runtime' or '--target' options."
                        .to_string()
                )
            );
        }
    }

    #[test]
    fn compiler_offsets_only_include_selected_checkers() {
        let own_offsets = [30, 10];
        let rt_offsets = [20, 10];
        assert_eq!(
            selected_compiler_offsets(CheckerSelection::All, &own_offsets, &rt_offsets),
            vec![10, 20, 30]
        );
        assert_eq!(
            selected_compiler_offsets(CheckerSelection::Own, &own_offsets, &rt_offsets),
            vec![10, 30]
        );
        assert_eq!(
            selected_compiler_offsets(CheckerSelection::Rt, &own_offsets, &rt_offsets),
            vec![10, 20]
        );
    }

    #[test]
    fn compiler_resolution_respects_selection_sparse_evidence_and_explicit_on() {
        assert!(!should_resolve_compiler(
            CheckerSelection::Own,
            &[],
            &CompilerMode::Auto
        ));
        assert!(should_resolve_compiler(
            CheckerSelection::Own,
            &[10],
            &CompilerMode::Auto
        ));
        assert!(should_resolve_compiler(
            CheckerSelection::Rt,
            &[],
            &CompilerMode::Auto
        ));
        assert!(should_resolve_compiler(
            CheckerSelection::Own,
            &[],
            &CompilerMode::On(CompilerOptions {
                corsa_path: None,
                tsconfig_path: None,
            })
        ));
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
                    checker: CheckerSelection::All,
                    runtime: Runtime::Bun,
                    environment: Environment::Node,
                    compiler: CompilerMode::Auto,
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
                    checker: CheckerSelection::All,
                    runtime: Runtime::None,
                    environment: Environment::Bun,
                    compiler: CompilerMode::Auto,
                },
            })
        );
    }

    #[test]
    fn compiler_defaults_to_auto() {
        assert_eq!(
            parse_args(&args(&["check", "input.js"])).unwrap(),
            Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    checker: CheckerSelection::All,
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: CompilerMode::Auto,
                },
            }
        );
    }

    #[test]
    fn no_corsa_disables_compiler() {
        assert_eq!(
            parse_args(&args(&["check", "--no-corsa", "input.js"])).unwrap(),
            Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    checker: CheckerSelection::All,
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: CompilerMode::Off,
                },
            }
        );
    }

    #[test]
    fn compiler_flags_are_optional_overrides() {
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
                    checker: CheckerSelection::All,
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: CompilerMode::On(CompilerOptions {
                        corsa_path: Some("/tools/corsa".to_string()),
                        tsconfig_path: Some("/project/tsconfig.json".to_string()),
                    }),
                },
            })
        );
        assert_eq!(
            parse_args(&args(&["check", "--corsa", "/tools/corsa", "input.js"])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    checker: CheckerSelection::All,
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: CompilerMode::On(CompilerOptions {
                        corsa_path: Some("/tools/corsa".to_string()),
                        tsconfig_path: None,
                    }),
                },
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "check",
                "--tsconfig",
                "/project/tsconfig.json",
                "input.js",
            ])),
            Ok(Command::Check {
                paths: vec![PathBuf::from("input.js")],
                options: CheckOptions {
                    checker: CheckerSelection::All,
                    runtime: Runtime::default(),
                    environment: Environment::Auto,
                    compiler: CompilerMode::On(CompilerOptions {
                        corsa_path: None,
                        tsconfig_path: Some("/project/tsconfig.json".to_string()),
                    }),
                },
            })
        );
    }

    #[test]
    fn auto_compiler_skips_missing_source() {
        assert!(compiler_for_file("no-such-file.js", &CompilerMode::Auto)
            .unwrap()
            .is_none());
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

    #[test]
    fn finds_nearest_tsconfig_from_source() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../rt/fixtures/compiler/project/entry.js");
        let found = find_tsconfig(&source).expect("tsconfig next to fixture");
        assert!(
            found.ends_with("compiler/project/tsconfig.json"),
            "{}",
            found.display()
        );
    }
}
