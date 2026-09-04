//! Reproducible integration matrix for checker, compiler, and platform axes.

use pragma_loc::{byte_offset_to_line_col, utf16_offset_to_line_col};
use pragma_own::Runtime;
use pragma_parse::{diagnostic_span, parse, Allocator, ParseDiagnostic};
use pragma_rt::prelude::Environment;
use pragma_rt::type_provider::{
    utf16_offset_for_byte_offset, CompilerDiagnostic, CompilerDiagnosticKind,
    CompilerDiagnosticSeverity, CompilerRange, CompilerTypeAnalysis, CompilerTypeAtOffset,
    CompilerTypeProvider, CompilerTypeProviderError, CompilerTypeRequest,
};
use pragmajs::{
    check_parsed, check_parsed_with_compiler_provider, CheckOptions, CheckerSelection,
    CombinedCheck, CombinedError, CompilerMode, CompilerOptions, PlatformProfile,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompilerCell {
    Off,
    Auto,
    ExplicitFixture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Counts {
    parse: usize,
    own: usize,
    rt: usize,
    compiler: usize,
    provider: usize,
}

#[derive(Debug)]
struct Cell {
    name: String,
    fixture: PathBuf,
    checker: CheckerSelection,
    compiler: CompilerCell,
    platform: Option<PlatformProfile>,
    runtime: Runtime,
    target: Environment,
    expected: Counts,
    expected_failed: bool,
    expected_locations: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ObservedDiagnostic {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl ObservedDiagnostic {
    fn without_location(message: String) -> Self {
        Self {
            message,
            line: None,
            column: None,
        }
    }

    fn at(message: String, line: u32, column: u32) -> Self {
        Self {
            message,
            line: Some(line),
            column: Some(column),
        }
    }

    fn render(&self) -> String {
        match (self.line, self.column) {
            (Some(line), Some(column)) => format!("{line}:{column}: {}", self.message),
            _ => self.message.clone(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Observation {
    pub name: String,
    pub fixture: String,
    pub checker: String,
    pub compiler: String,
    pub platform: String,
    pub runtime: String,
    pub target: String,
    pub parse_diagnostics: Vec<ObservedDiagnostic>,
    pub own_diagnostics: Vec<ObservedDiagnostic>,
    pub rt_diagnostics: Vec<ObservedDiagnostic>,
    pub compiler_diagnostics: Vec<ObservedDiagnostic>,
    pub provider_errors: Vec<ObservedDiagnostic>,
    pub frontend_parse_count: usize,
    pub elapsed_micros: u128,
    pub combined_failed: bool,
    pub matches_gold: bool,
}

impl Observation {
    fn counts(&self) -> Counts {
        Counts {
            parse: self.parse_diagnostics.len(),
            own: self.own_diagnostics.len(),
            rt: self.rt_diagnostics.len(),
            compiler: self.compiler_diagnostics.len(),
            provider: self.provider_errors.len(),
        }
    }

    fn location_gold(&self) -> String {
        fn producer(name: &str, diagnostics: &[ObservedDiagnostic]) -> Option<String> {
            (!diagnostics.is_empty()).then(|| {
                let locations = diagnostics
                    .iter()
                    .map(|diagnostic| match (diagnostic.line, diagnostic.column) {
                        (Some(line), Some(column)) => format!("{line}:{column}"),
                        _ => "?:?".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{name}={locations}")
            })
        }

        [
            producer("parse", &self.parse_diagnostics),
            producer("own", &self.own_diagnostics),
            producer("rt", &self.rt_diagnostics),
            producer("compiler", &self.compiler_diagnostics),
            producer("provider", &self.provider_errors),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(";")
    }
}

struct FixtureProvider;

impl CompilerTypeProvider for FixtureProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        if request.source.contains("@ablation-provider-error") {
            return Err(CompilerTypeProviderError::InvalidCompilerOptions {
                config: request.config_path.clone(),
                message: "deterministic provider failure".to_string(),
            });
        }
        let rendered_type = if request.source.contains("@ablation-compiler-buffer") {
            Some("Buffer")
        } else if request.source.contains("@ablation-compiler-number") {
            Some("number")
        } else {
            None
        };
        let definition_offsets = request
            .definition_byte_offsets
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let types = request
            .byte_offsets
            .iter()
            .map(|byte_offset| {
                Ok(CompilerTypeAtOffset {
                    byte_offset: *byte_offset,
                    utf16_offset: utf16_offset_for_byte_offset(&request.source, *byte_offset)?,
                    rendered_type: rendered_type.map(str::to_string),
                    call_return_types: Vec::new(),
                    definition_paths: definition_offsets
                        .contains(byte_offset)
                        .then(|| "file:///typescript/lib/lib.es2025.d.ts".to_string())
                        .into_iter()
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>, CompilerTypeProviderError>>()?;
        let diagnostics = request
            .source
            .find("@ablation-compiler-error")
            .map(|offset| {
                Ok(CompilerDiagnostic {
                    file: request.file_path.to_string_lossy().into_owned(),
                    kind: CompilerDiagnosticKind::Semantic,
                    severity: CompilerDiagnosticSeverity::Error,
                    code: Some("9999".to_string()),
                    source: Some("ablation-fixture".to_string()),
                    message: "deterministic compiler diagnostic".to_string(),
                    range: CompilerRange {
                        start_utf16: utf16_offset_for_byte_offset(&request.source, offset)?,
                        end_utf16: utf16_offset_for_byte_offset(
                            &request.source,
                            offset + "@ablation-compiler-error".len(),
                        )?,
                    },
                })
            })
            .transpose()?
            .into_iter()
            .collect();
        Ok(CompilerTypeAnalysis { types, diagnostics })
    }
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ablation")
}

fn parse_checker(value: &str) -> Result<CheckerSelection, String> {
    match value {
        "own" => Ok(CheckerSelection::Own),
        "rt" => Ok(CheckerSelection::Rt),
        "all" => Ok(CheckerSelection::All),
        _ => Err(format!("unknown checker `{value}`")),
    }
}

fn parse_compiler(value: &str) -> Result<CompilerCell, String> {
    match value {
        "off" => Ok(CompilerCell::Off),
        "auto" => Ok(CompilerCell::Auto),
        "explicit" => Ok(CompilerCell::ExplicitFixture),
        _ => Err(format!("unknown compiler mode `{value}`")),
    }
}

fn parse_platform(value: &str) -> Result<Option<PlatformProfile>, String> {
    if value == "-" {
        return Ok(None);
    }
    PlatformProfile::parse(value)
        .map(Some)
        .ok_or_else(|| format!("unknown platform profile `{value}`"))
}

fn parse_runtime(value: &str) -> Result<Runtime, String> {
    Runtime::parse(value).ok_or_else(|| format!("unknown runtime `{value}`"))
}

fn parse_target(value: &str) -> Result<Environment, String> {
    match value {
        "auto" => Ok(Environment::Auto),
        "ecmascript" => Ok(Environment::Ecmascript),
        "browser" => Ok(Environment::Browser),
        "node" => Ok(Environment::Node),
        "deno" => Ok(Environment::Deno),
        "bun" => Ok(Environment::Bun),
        _ => Err(format!("unknown target `{value}`")),
    }
}

fn parse_count(value: &str, line: usize, column: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("manifest line {line} has invalid {column} count `{value}`"))
}

fn load_cells() -> Result<Vec<Cell>, String> {
    let root = fixture_root();
    let manifest_path = root.join("manifest.tsv");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let mut cells = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_number = index + 1;
        let fields = line.split('\t').collect::<Vec<_>>();
        if !(13..=14).contains(&fields.len()) {
            return Err(format!(
                "manifest line {line_number} has {} fields; expected 13 or 14",
                fields.len()
            ));
        }
        let fixture = root.join(fields[1]);
        if !fixture.is_file() {
            return Err(format!(
                "manifest line {line_number} fixture does not exist: {}",
                fixture.display()
            ));
        }
        let platform = parse_platform(fields[4])?;
        let (runtime, target) = if let Some(profile) = platform {
            if fields[5] != "-" || fields[6] != "-" {
                return Err(format!(
                    "manifest line {line_number} cannot combine a platform profile with runtime/target overrides"
                ));
            }
            profile.settings()
        } else {
            (parse_runtime(fields[5])?, parse_target(fields[6])?)
        };
        cells.push(Cell {
            name: fields[0].to_string(),
            fixture,
            checker: parse_checker(fields[2])?,
            compiler: parse_compiler(fields[3])?,
            platform,
            runtime,
            target,
            expected: Counts {
                parse: parse_count(fields[7], line_number, "parse")?,
                own: parse_count(fields[8], line_number, "own")?,
                rt: parse_count(fields[9], line_number, "rt")?,
                compiler: parse_count(fields[10], line_number, "compiler")?,
                provider: parse_count(fields[11], line_number, "provider")?,
            },
            expected_failed: fields[12]
                .parse()
                .map_err(|_| format!("manifest line {line_number} has invalid failed boolean"))?,
            expected_locations: fields.get(13).map(|value| (*value).to_string()),
        });
    }
    Ok(cells)
}

fn options(cell: &Cell) -> CheckOptions {
    let compiler = match cell.compiler {
        CompilerCell::Off => CompilerMode::Off,
        CompilerCell::Auto => CompilerMode::Auto,
        CompilerCell::ExplicitFixture => CompilerMode::On(CompilerOptions {
            corsa_path: None,
            tsconfig_path: None,
        }),
    };
    let options = CheckOptions {
        checker: cell.checker,
        runtime: cell.runtime,
        environment: cell.target,
        compiler,
    };
    match cell.platform {
        Some(platform) => options.with_platform(platform),
        None => options,
    }
}

fn checker_name(checker: CheckerSelection) -> &'static str {
    match checker {
        CheckerSelection::Own => "own",
        CheckerSelection::Rt => "rt",
        CheckerSelection::All => "all",
    }
}

fn compiler_name(compiler: CompilerCell) -> &'static str {
    match compiler {
        CompilerCell::Off => "off",
        CompilerCell::Auto => "auto",
        CompilerCell::ExplicitFixture => "explicit-fixture",
    }
}

fn runtime_name(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Node => "node",
        Runtime::Bun => "bun",
        Runtime::Deno => "deno",
        Runtime::None => "none",
    }
}

fn target_name(target: Environment) -> &'static str {
    match target {
        Environment::Auto => "auto",
        Environment::Ecmascript => "ecmascript",
        Environment::Browser => "browser",
        Environment::Node => "node",
        Environment::Deno => "deno",
        Environment::Bun => "bun",
    }
}

fn observe_parse_diagnostic(source: &str, diagnostic: &ParseDiagnostic) -> ObservedDiagnostic {
    let message = format!("{diagnostic:?}");
    if let Some(span) = diagnostic_span(diagnostic) {
        let (line, column) = byte_offset_to_line_col(source, span.start);
        ObservedDiagnostic::at(message, line, column)
    } else {
        ObservedDiagnostic::without_location(message)
    }
}

fn observe_check(
    cell: &Cell,
    source: &str,
    check: CombinedCheck,
    frontend_parse_count: usize,
    elapsed_micros: u128,
) -> Observation {
    let combined_failed = check.failed();
    let rt_diagnostics = check
        .refinement_diagnostics()
        .iter()
        .map(|diagnostic| match &diagnostic.loc {
            Some(location) => {
                ObservedDiagnostic::at(diagnostic.message.clone(), location.line, location.column)
            }
            None => ObservedDiagnostic::without_location(diagnostic.message.clone()),
        })
        .collect();
    let compiler_diagnostics = check
        .compiler_diagnostics
        .iter()
        .map(|diagnostic| {
            let message = format!(
                "{:?}:TS{}: {}",
                diagnostic.kind,
                diagnostic.code.as_deref().unwrap_or("?"),
                diagnostic.message
            );
            let (line, column) = utf16_offset_to_line_col(source, diagnostic.range.start_utf16);
            ObservedDiagnostic::at(message, line, column)
        })
        .collect();
    let own_diagnostics = check
        .own
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let (line, column) = diagnostic.line_col(source);
            ObservedDiagnostic::at(
                format!("{}: {}", diagnostic.kind.slug(), diagnostic.message),
                line,
                column,
            )
        })
        .collect();
    let parse_diagnostics = check
        .parse_diagnostics
        .iter()
        .map(|diagnostic| observe_parse_diagnostic(source, diagnostic))
        .collect();
    observation(
        cell,
        parse_diagnostics,
        own_diagnostics,
        rt_diagnostics,
        compiler_diagnostics,
        Vec::new(),
        frontend_parse_count,
        elapsed_micros,
        combined_failed,
    )
}

#[allow(clippy::too_many_arguments)]
fn observation(
    cell: &Cell,
    parse_diagnostics: Vec<ObservedDiagnostic>,
    own_diagnostics: Vec<ObservedDiagnostic>,
    rt_diagnostics: Vec<ObservedDiagnostic>,
    compiler_diagnostics: Vec<ObservedDiagnostic>,
    provider_errors: Vec<ObservedDiagnostic>,
    frontend_parse_count: usize,
    elapsed_micros: u128,
    combined_failed: bool,
) -> Observation {
    let mut observation = Observation {
        name: cell.name.clone(),
        fixture: cell
            .fixture
            .file_name()
            .expect("validated fixture path")
            .to_string_lossy()
            .into_owned(),
        checker: checker_name(cell.checker).to_string(),
        compiler: compiler_name(cell.compiler).to_string(),
        platform: cell
            .platform
            .map(PlatformProfile::as_str)
            .unwrap_or("-")
            .to_string(),
        runtime: runtime_name(cell.runtime).to_string(),
        target: target_name(cell.target).to_string(),
        parse_diagnostics,
        own_diagnostics,
        rt_diagnostics,
        compiler_diagnostics,
        provider_errors,
        frontend_parse_count,
        elapsed_micros,
        combined_failed,
        matches_gold: false,
    };
    observation.matches_gold = observation.counts() == cell.expected
        && observation.combined_failed == cell.expected_failed
        && cell
            .expected_locations
            .as_ref()
            .is_none_or(|expected| observation.location_gold() == *expected);
    observation
}

fn evaluate(cell: &Cell) -> Result<Observation, String> {
    let started = Instant::now();
    let source = fs::read_to_string(&cell.fixture)
        .map_err(|error| format!("{}: {error}", cell.fixture.display()))?;
    let filename = cell.fixture.to_string_lossy();
    let options = options(cell);
    let allocator = Allocator::new();
    let parsed = parse(&allocator, &filename, &source);
    let result = match cell.compiler {
        CompilerCell::Off | CompilerCell::Auto => {
            check_parsed(&filename, &source, &parsed, &options)
        }
        CompilerCell::ExplicitFixture => check_parsed_with_compiler_provider(
            &filename,
            &source,
            &parsed,
            &options,
            &FixtureProvider,
            &fixture_root().join("tsconfig.json"),
            &cell.fixture,
        ),
    };
    let elapsed_micros = started.elapsed().as_micros();
    match result {
        Ok(check) => Ok(observe_check(cell, &source, check, 1, elapsed_micros)),
        Err(CombinedError::Compiler(error)) => {
            let root = fixture_root().to_string_lossy().into_owned();
            Ok(observation(
                cell,
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| observe_parse_diagnostic(&source, diagnostic))
                    .collect(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![ObservedDiagnostic::without_location(
                    error.to_string().replace(&root, "<fixture-root>"),
                )],
                1,
                elapsed_micros,
                true,
            ))
        }
        Err(error) => Err(format!("{}: {error}", cell.name)),
    }
}

pub fn run_matrix() -> Result<Vec<Observation>, String> {
    load_cells()?.iter().map(evaluate).collect()
}

fn csv(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn details(values: &[ObservedDiagnostic]) -> String {
    values
        .iter()
        .map(ObservedDiagnostic::render)
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn render_csv(observations: &[Observation]) -> String {
    let mut lines = vec!["name,fixture,checker,compiler,platform,runtime,target,frontend_parse_count,elapsed_micros,parse_diagnostic_count,own_diagnostic_count,rt_diagnostic_count,compiler_diagnostic_count,provider_error_count,combined_failed,matches_gold,parse_diagnostics,own_diagnostics,rt_diagnostics,compiler_diagnostics,provider_errors".to_string()];
    for observation in observations {
        let counts = observation.counts();
        lines.push(format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv(&observation.name),
            csv(&observation.fixture),
            observation.checker,
            observation.compiler,
            observation.platform,
            observation.runtime,
            observation.target,
            observation.frontend_parse_count,
            observation.elapsed_micros,
            counts.parse,
            counts.own,
            counts.rt,
            counts.compiler,
            counts.provider,
            observation.combined_failed,
            observation.matches_gold,
            csv(&details(&observation.parse_diagnostics)),
            csv(&details(&observation.own_diagnostics)),
            csv(&details(&observation.rt_diagnostics)),
            csv(&details(&observation.compiler_diagnostics)),
            csv(&details(&observation.provider_errors)),
        ));
    }
    lines.join("\n")
}

#[allow(dead_code)]
fn main() -> Result<(), String> {
    let observations = run_matrix()?;
    println!("{}", render_csv(&observations));
    let mismatches = observations
        .iter()
        .filter(|observation| !observation.matches_gold)
        .map(|observation| observation.name.as_str())
        .collect::<Vec<_>>();
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(format!("gold mismatches: {}", mismatches.join(", ")))
    }
}
