//! Compiler-backed TypeScript type information.
//!
//! This boundary deliberately owns no executable discovery or project
//! inference. Callers provide the exact Corsa executable, working directory,
//! `tsconfig`, source file, and UTF-8 byte offsets they want analyzed. Before
//! returning compiler evidence, the provider verifies Corsa's normalized
//! compiler options, rejects source-level diagnostic suppression, and records
//! each queried symbol's declaring files for provenance checks.

use std::{collections::BTreeSet, fs, io, path::PathBuf};

use corsa::{
    api::{
        ApiMode, ApiSpawnConfig, DocumentIdentifier, ProjectSession, SignatureHandle,
        SymbolResponse, TypeResponse,
    },
    runtime::block_on,
};
use pragma_parse::{parse, Allocator};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

// TypeScript's public SymbolFlags.Alias bit. Corsa exposes the raw bitset so
// imported aliases can be resolved to their actual declaring files.
const SYMBOL_FLAGS_ALIAS: u32 = 1 << 21;

/// A compiler type query for one source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilerTypeRequest {
    /// Absolute path to the project configuration Corsa should open.
    pub config_path: PathBuf,
    /// Absolute path to the source file whose offsets are queried.
    pub file_path: PathBuf,
    /// Exact source text expected to match the on-disk file Corsa opens.
    pub source: String,
    /// UTF-8 byte offsets in caller order. Duplicate offsets are preserved.
    pub byte_offsets: Vec<usize>,
    /// Subset of `byte_offsets` whose types should be inspected for call
    /// signatures. Other offsets only request the rendered type.
    pub callable_byte_offsets: Vec<usize>,
    /// Subset of `byte_offsets` whose symbol definitions should be resolved.
    /// The checker uses this provenance to distinguish declaration-backed
    /// library evidence from unverified project implementations.
    pub definition_byte_offsets: Vec<usize>,
}

/// Result of one requested source offset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilerTypeAtOffset {
    /// Original UTF-8 byte offset supplied by the caller.
    pub byte_offset: usize,
    /// Equivalent absolute UTF-16 code-unit offset sent to Corsa.
    pub utf16_offset: u32,
    /// Compiler-rendered TypeScript type, or `None` when the position has no type.
    pub rendered_type: Option<String>,
    /// Compiler-rendered return types for call signatures on the queried type.
    ///
    /// Values are sorted and deduplicated so callers do not depend on compiler
    /// overload enumeration order.
    pub call_return_types: Vec<String>,
    /// Declaring file paths reported for this position, sorted and deduplicated.
    /// An empty list means that the provider could not establish provenance.
    pub definition_paths: Vec<String>,
}

/// TypeScript diagnostic group reported by Corsa.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CompilerDiagnosticKind {
    Config,
    Program,
    Global,
    Syntactic,
    Bind,
    Semantic,
    Suggestion,
}

/// Severity copied into a refinejs-owned representation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CompilerDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
    Unspecified,
}

/// A half-open range expressed as absolute UTF-16 code-unit offsets.
///
/// Corsa represents diagnostics without a source range as `-1..-1`. Those
/// project-wide diagnostics use the empty sentinel `0..0` here because this
/// refinejs-owned compatibility type predates optional diagnostic ranges.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CompilerRange {
    pub start_utf16: u32,
    pub end_utf16: u32,
}

/// A compiler diagnostic detached from Corsa and LSP transport types.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CompilerDiagnostic {
    /// Compiler file name, or the requested config path when Corsa reports a
    /// project-wide diagnostic without a file.
    pub file: String,
    pub kind: CompilerDiagnosticKind,
    pub severity: CompilerDiagnosticSeverity,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub range: CompilerRange,
}

/// All compiler evidence collected with one project session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilerTypeAnalysis {
    pub types: Vec<CompilerTypeAtOffset>,
    pub diagnostics: Vec<CompilerDiagnostic>,
}

/// Stable boundary used by the refinement checker to request compiler types.
pub trait CompilerTypeProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError>;
}

/// Corsa-backed provider configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorsaTypeProvider {
    executable: PathBuf,
    working_directory: PathBuf,
}

impl CorsaTypeProvider {
    /// Create a provider with explicit paths and no executable fallback.
    pub fn new(executable: impl Into<PathBuf>, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            working_directory: working_directory.into(),
        }
    }

    /// Analyze all requested positions in one Corsa project session.
    pub fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        self.validate_paths(request)?;
        self.ensure_source_matches(request)?;
        reject_diagnostic_suppression(&request.source, &request.file_path)?;
        for &byte_offset in &request.callable_byte_offsets {
            if !request.byte_offsets.contains(&byte_offset) {
                return Err(CompilerTypeProviderError::CallableOffsetNotQueried {
                    file: request.file_path.clone(),
                    byte_offset,
                });
            }
        }
        for &byte_offset in &request.definition_byte_offsets {
            if !request.byte_offsets.contains(&byte_offset) {
                return Err(CompilerTypeProviderError::DefinitionOffsetNotQueried {
                    file: request.file_path.clone(),
                    byte_offset,
                });
            }
        }
        let utf16_offsets = request
            .byte_offsets
            .iter()
            .copied()
            .map(|byte_offset| {
                utf16_offset_for_byte_offset(&request.source, byte_offset)
                    .map_err(|error| error.with_file(request.file_path.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let executable = path_as_utf8(&self.executable, "Corsa executable")?;
        let working_directory = path_as_utf8(&self.working_directory, "working directory")?;
        let config_path = path_as_utf8(&request.config_path, "TypeScript config")?;
        let file_path = path_as_utf8(&request.file_path, "source file")?;

        block_on(self.analyze_with_session(
            request,
            executable,
            working_directory,
            config_path,
            file_path,
            utf16_offsets,
        ))
    }

    async fn analyze_with_session(
        &self,
        request: &CompilerTypeRequest,
        executable: String,
        working_directory: String,
        config_path: String,
        file_path: String,
        utf16_offsets: Vec<u32>,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        let session = ProjectSession::spawn(
            ApiSpawnConfig::new(executable)
                .with_cwd(working_directory)
                .with_mode(ApiMode::SyncMsgpackStdio),
            config_path.clone(),
            Some(DocumentIdentifier::from(file_path.clone())),
        )
        .await
        .map_err(|source| self.corsa_error(request, "start project session", source))?;

        let analysis = async {
            // `parseConfigFile` returns options after `extends` resolution. The
            // soundness guard must run before any queried type is trusted.
            let config = session
                .client()
                .parse_config_file(DocumentIdentifier::from(config_path))
                .await
                .map_err(|source| {
                    self.corsa_error(request, "parse normalized compiler options", source)
                })?;
            let initialize = session.client().initialize().await.map_err(|source| {
                self.corsa_error(request, "read compiler path semantics", source)
            })?;
            require_resolved_project_config(
                &session.project().config_file_name,
                &request.config_path,
                std::path::Path::new(&initialize.current_directory),
                initialize.use_case_sensitive_file_names,
            )?;
            require_source_in_project(
                &config.file_names,
                &request.file_path,
                std::path::Path::new(&initialize.current_directory),
                initialize.use_case_sensitive_file_names,
                &request.config_path,
            )?;
            validate_sound_compiler_options(&config.options, request)?;
            let program_files = self.query_program_file_names(&session, request).await?;
            reject_program_diagnostic_suppression(
                &program_files,
                std::path::Path::new(&initialize.current_directory),
            )?;
            let analysis = self
                .query_session(&session, request, file_path, utf16_offsets)
                .await?;
            reject_program_diagnostic_suppression(
                &program_files,
                std::path::Path::new(&initialize.current_directory),
            )?;
            Ok(analysis)
        }
        .await;
        let release_error = session.snapshot().release().await.err();
        let close_error = session.close().await.err();

        let analysis = analysis?;
        self.ensure_source_matches(request)?;
        if let Some(source) = release_error {
            return Err(self.corsa_error(request, "release project snapshot", source));
        }
        if let Some(source) = close_error {
            return Err(self.corsa_error(request, "close project session", source));
        }
        Ok(analysis)
    }

    async fn query_session(
        &self,
        session: &ProjectSession,
        request: &CompilerTypeRequest,
        file_path: String,
        utf16_offsets: Vec<u32>,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        let responses = session
            .client()
            .get_types_at_positions(
                session.snapshot().handle.clone(),
                session.project_handle(),
                file_path.clone(),
                utf16_offsets.clone(),
            )
            .await
            .map_err(|source| self.corsa_error(request, "query types at positions", source))?;

        if responses.len() != request.byte_offsets.len() {
            return Err(CompilerTypeProviderError::ResponseLengthMismatch {
                file: request.file_path.clone(),
                requested: request.byte_offsets.len(),
                returned: responses.len(),
            });
        }

        let utf16_by_byte = request
            .byte_offsets
            .iter()
            .copied()
            .zip(utf16_offsets.iter().copied())
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut definitions_by_byte = std::collections::BTreeMap::new();
        let definition_offsets = request
            .definition_byte_offsets
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let definition_positions = definition_offsets
            .iter()
            .map(|byte_offset| utf16_by_byte[byte_offset])
            .collect();
        let symbols = session
            .client()
            .get_symbols_at_positions(
                session.snapshot().handle.clone(),
                session.project_handle(),
                file_path.clone(),
                definition_positions,
            )
            .await
            .map_err(|source| self.corsa_error(request, "query symbols at positions", source))?;
        if symbols.len() != definition_offsets.len() {
            return Err(
                CompilerTypeProviderError::DefinitionResponseLengthMismatch {
                    file: request.file_path.clone(),
                    requested: definition_offsets.len(),
                    returned: symbols.len(),
                },
            );
        }
        for (byte_offset, symbol) in definition_offsets.into_iter().zip(symbols) {
            let symbol = if let Some(symbol) = symbol {
                if symbol.flags & SYMBOL_FLAGS_ALIAS != 0 {
                    session
                        .client()
                        .get_aliased_symbol(
                            session.snapshot().handle.clone(),
                            session.project_handle(),
                            symbol.id.clone(),
                        )
                        .await
                        .map_err(|source| {
                            self.corsa_error(request, "resolve aliased symbol", source)
                        })?
                        .or(Some(symbol))
                } else {
                    Some(symbol)
                }
            } else {
                None
            };
            definitions_by_byte.insert(byte_offset, definition_paths(symbol));
        }

        let mut types = Vec::with_capacity(responses.len());
        let callable_byte_offsets = request
            .callable_byte_offsets
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for ((byte_offset, utf16_offset), response) in request
            .byte_offsets
            .iter()
            .copied()
            .zip(utf16_offsets)
            .zip(responses)
        {
            let (rendered_type, call_return_types) = match response {
                Some(response) => {
                    let mut call_return_types = Vec::new();
                    if callable_byte_offsets.contains(&byte_offset) {
                        let signatures = session
                            .get_signatures_of_type(response.id.clone(), 0)
                            .await
                            .map_err(|source| {
                                self.corsa_error(request, "query call signatures", source)
                            })?;
                        for signature in signatures {
                            let Some(return_type) = self
                                .query_call_return_type(session, request, &signature.id)
                                .await?
                            else {
                                continue;
                            };
                            call_return_types.push(
                                session
                                    .type_to_string(return_type.id, None, None)
                                    .await
                                    .map_err(|source| {
                                        self.corsa_error(
                                            request,
                                            "render call signature return type",
                                            source,
                                        )
                                    })?,
                            );
                        }
                    }
                    call_return_types.sort();
                    call_return_types.dedup();
                    let rendered_type = session
                        .type_to_string(response.id, None, None)
                        .await
                        .map_err(|source| {
                            self.corsa_error(request, "render queried type", source)
                        })?;
                    (Some(rendered_type), call_return_types)
                }
                None => (None, Vec::new()),
            };
            types.push(CompilerTypeAtOffset {
                byte_offset,
                utf16_offset,
                rendered_type,
                call_return_types,
                definition_paths: definitions_by_byte
                    .get(&byte_offset)
                    .cloned()
                    .unwrap_or_default(),
            });
        }

        let diagnostics = self.query_diagnostics(session, request).await?;

        Ok(CompilerTypeAnalysis { types, diagnostics })
    }

    async fn query_program_file_names(
        &self,
        session: &ProjectSession,
        request: &CompilerTypeRequest,
    ) -> Result<Vec<String>, CompilerTypeProviderError> {
        let snapshot = parse_numeric_handle("snapshot", session.snapshot().handle.as_str())?;
        let value = session
            .client()
            .raw_json_request(
                "getSourceFileNames",
                json!({
                    "snapshot": snapshot,
                    "project": session.project_handle().as_str(),
                }),
            )
            .await
            .map_err(|source| {
                self.corsa_error(request, "enumerate program source files", source)
            })?;
        serde_json::from_value(value).map_err(|source| CompilerTypeProviderError::DecodeResponse {
            method: "getSourceFileNames",
            file: request.file_path.clone(),
            source,
        })
    }

    async fn query_call_return_type(
        &self,
        session: &ProjectSession,
        request: &CompilerTypeRequest,
        signature: &SignatureHandle,
    ) -> Result<Option<TypeResponse>, CompilerTypeProviderError> {
        // Corsa 1.12.4's typed helper does not mirror `signature` to the
        // upstream API's stable `objectId` field. Use the low-level endpoint
        // explicitly while sending both keys for protocol compatibility.
        let snapshot = parse_numeric_handle("snapshot", session.snapshot().handle.as_str())?;
        let signature = parse_numeric_handle("signature", signature.as_str())?;
        let value = session
            .client()
            .raw_json_request(
                "getReturnTypeOfSignature",
                json!({
                    "snapshot": snapshot,
                    "project": session.project_handle().as_str(),
                    "objectId": signature,
                    "signature": signature,
                }),
            )
            .await
            .map_err(|source| {
                self.corsa_error(request, "query call signature return type", source)
            })?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|source| {
            CompilerTypeProviderError::DecodeResponse {
                method: "getReturnTypeOfSignature",
                file: request.file_path.clone(),
                source,
            }
        })
    }

    async fn query_diagnostics(
        &self,
        session: &ProjectSession,
        request: &CompilerTypeRequest,
    ) -> Result<Vec<CompilerDiagnostic>, CompilerTypeProviderError> {
        // Corsa 1.12.4 does not expose typed wrappers for these stable upstream
        // endpoints. Omitting `files` from the four file-diagnostic requests is
        // the upstream API's explicit all-project form; scoping them to the
        // requested source would let errors in imported project files hide
        // behind an otherwise usable type response.
        let snapshot = parse_numeric_handle("snapshot", session.snapshot().handle.as_str())?;
        let config_path = path_as_utf8(&request.config_path, "TypeScript config")?;
        let groups = project_diagnostic_queries(
            snapshot,
            session.project_handle().as_str(),
            config_path.as_str(),
        );
        let mut diagnostics = Vec::new();

        for (method, kind, params, fallback_file) in groups {
            let value = session
                .client()
                .raw_json_request(method, params)
                .await
                .map_err(|source| self.corsa_error(request, method, source))?;
            let raw_diagnostics = if value.is_null() {
                Vec::new()
            } else {
                serde_json::from_value::<Vec<CorsaDiagnostic>>(value).map_err(|source| {
                    CompilerTypeProviderError::DecodeResponse {
                        method,
                        file: request.file_path.clone(),
                        source,
                    }
                })?
            };
            for diagnostic in raw_diagnostics {
                diagnostics.push(convert_diagnostic(diagnostic, kind, fallback_file)?);
            }
        }

        sort_and_deduplicate_diagnostics(&mut diagnostics);
        Ok(diagnostics)
    }

    fn validate_paths(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<(), CompilerTypeProviderError> {
        require_absolute(&self.executable, "Corsa executable")?;
        require_absolute(&self.working_directory, "working directory")?;
        require_absolute(&request.config_path, "TypeScript config")?;
        require_absolute(&request.file_path, "source file")?;

        if !self.executable.is_file() {
            return Err(CompilerTypeProviderError::MissingExecutable {
                path: self.executable.clone(),
            });
        }
        if !self.working_directory.is_dir() {
            return Err(CompilerTypeProviderError::MissingWorkingDirectory {
                path: self.working_directory.clone(),
            });
        }
        if !request.config_path.is_file() {
            return Err(CompilerTypeProviderError::MissingConfig {
                path: request.config_path.clone(),
            });
        }
        if !request.file_path.is_file() {
            return Err(CompilerTypeProviderError::MissingSource {
                path: request.file_path.clone(),
            });
        }
        Ok(())
    }

    fn ensure_source_matches(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<(), CompilerTypeProviderError> {
        let disk_source = fs::read_to_string(&request.file_path).map_err(|source| {
            CompilerTypeProviderError::ReadSource {
                path: request.file_path.clone(),
                source,
            }
        })?;
        if disk_source == request.source {
            Ok(())
        } else {
            Err(CompilerTypeProviderError::SourceMismatch {
                path: request.file_path.clone(),
            })
        }
    }

    fn corsa_error(
        &self,
        request: &CompilerTypeRequest,
        operation: &'static str,
        source: corsa::CorsaError,
    ) -> CompilerTypeProviderError {
        CompilerTypeProviderError::Corsa {
            operation,
            executable: self.executable.clone(),
            config: request.config_path.clone(),
            file: request.file_path.clone(),
            source: Box::new(source),
        }
    }
}

fn definition_paths(response: Option<SymbolResponse>) -> Vec<String> {
    let mut paths = response.map_or_else(Vec::new, |symbol| {
        symbol
            .declarations
            .into_iter()
            .chain(symbol.value_declaration)
            .filter_map(|declaration| declaration.declaring_path())
            .map(|path| path.to_string())
            .collect()
    });
    paths.sort();
    paths.dedup();
    paths
}

/// Whether compiler evidence resolves exclusively to declaration files.
///
/// Missing definitions are rejected: refinement checking must not infer that
/// an opaque implementation is trustworthy merely because TypeScript printed
/// a useful type for it.
pub fn definitions_are_declaration_backed(paths: &[String]) -> bool {
    !paths.is_empty()
        && paths.iter().all(|path| {
            let path = path.to_ascii_lowercase();
            path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts")
        })
}

fn project_diagnostic_queries<'a>(
    snapshot: u64,
    project: &str,
    config_path: &'a str,
) -> Vec<(&'static str, CompilerDiagnosticKind, Value, &'a str)> {
    let project_params = json!({
        "snapshot": snapshot,
        "project": project,
    });
    vec![
        (
            "getConfigFileParsingDiagnostics",
            CompilerDiagnosticKind::Config,
            project_params.clone(),
            config_path,
        ),
        (
            "getProgramDiagnostics",
            CompilerDiagnosticKind::Program,
            project_params.clone(),
            config_path,
        ),
        (
            "getGlobalDiagnostics",
            CompilerDiagnosticKind::Global,
            project_params.clone(),
            config_path,
        ),
        (
            "getSyntacticDiagnostics",
            CompilerDiagnosticKind::Syntactic,
            project_params.clone(),
            config_path,
        ),
        (
            "getBindDiagnostics",
            CompilerDiagnosticKind::Bind,
            project_params.clone(),
            config_path,
        ),
        (
            "getSemanticDiagnostics",
            CompilerDiagnosticKind::Semantic,
            project_params.clone(),
            config_path,
        ),
        (
            "getSuggestionDiagnostics",
            CompilerDiagnosticKind::Suggestion,
            project_params,
            config_path,
        ),
    ]
}

impl CompilerTypeProvider for CorsaTypeProvider {
    fn analyze(
        &self,
        request: &CompilerTypeRequest,
    ) -> Result<CompilerTypeAnalysis, CompilerTypeProviderError> {
        CorsaTypeProvider::analyze(self, request)
    }
}

/// Convert a valid UTF-8 byte boundary into an absolute UTF-16 code-unit offset.
pub fn utf16_offset_for_byte_offset(
    source: &str,
    byte_offset: usize,
) -> Result<u32, CompilerTypeProviderError> {
    if byte_offset > source.len() {
        return Err(CompilerTypeProviderError::OffsetOutOfBounds {
            file: None,
            byte_offset,
            source_len: source.len(),
        });
    }
    if !source.is_char_boundary(byte_offset) {
        return Err(CompilerTypeProviderError::OffsetNotCharBoundary {
            file: None,
            byte_offset,
        });
    }
    let utf16_offset = source[..byte_offset].encode_utf16().count();
    u32::try_from(utf16_offset).map_err(|_| CompilerTypeProviderError::OffsetOverflow {
        file: None,
        byte_offset,
        utf16_offset,
    })
}

#[derive(Debug, Error)]
pub enum CompilerTypeProviderError {
    #[error("{role} path `{path}` must be absolute; supply the exact project path")]
    RelativePath { role: &'static str, path: PathBuf },
    #[error(
        "Corsa executable `{path}` is not a file; supply a TypeScript 7 binary with `--api` support"
    )]
    MissingExecutable { path: PathBuf },
    #[error("Corsa working directory `{path}` is not a directory")]
    MissingWorkingDirectory { path: PathBuf },
    #[error("TypeScript config `{path}` is not a file")]
    MissingConfig { path: PathBuf },
    #[error("source file `{path}` is not a file")]
    MissingSource { path: PathBuf },
    #[error("failed to read UTF-8 source file `{path}`: {source}")]
    ReadSource {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "source text for `{path}` differs from the on-disk file Corsa analyzes; reload the file and retry"
    )]
    SourceMismatch { path: PathBuf },
    #[error(
        "source file `{path}` contains TypeScript diagnostic-suppression directive `{directive}`; remove it before using compiler evidence"
    )]
    DiagnosticSuppression {
        path: PathBuf,
        directive: &'static str,
    },
    #[error(
        "callable byte offset {byte_offset} in `{file}` is not present in the requested byte offsets"
    )]
    CallableOffsetNotQueried { file: PathBuf, byte_offset: usize },
    #[error(
        "definition byte offset {byte_offset} in `{file}` is not present in the requested byte offsets"
    )]
    DefinitionOffsetNotQueried { file: PathBuf, byte_offset: usize },
    #[error(
        "source file `{file}` is not part of the file set resolved from TypeScript config `{config}`"
    )]
    SourceOutsideProject { config: PathBuf, file: PathBuf },
    #[error(
        "Corsa resolved source file through project config `{resolved}` instead of requested config `{requested}`"
    )]
    ResolvedProjectMismatch {
        requested: PathBuf,
        resolved: PathBuf,
    },
    #[error("Corsa returned invalid normalized compiler options for `{config}`: {message}")]
    InvalidCompilerOptions { config: PathBuf, message: String },
    #[error(
        "TypeScript config `{config}` enables `noCheck`; disable it before using compiler evidence"
    )]
    NoCheckEnabled { config: PathBuf },
    #[error(
        "TypeScript config `{config}` must enable `strictNullChecks` directly or through `strict` before using compiler evidence"
    )]
    StrictNullChecksDisabled { config: PathBuf },
    #[error(
        "JavaScript source `{file}` requires `checkJs: true` in TypeScript config `{config}` before using compiler evidence"
    )]
    CheckJsDisabled { config: PathBuf, file: PathBuf },
    #[error("{role} path `{path}` is not valid UTF-8 and cannot be sent to Corsa")]
    NonUtf8Path { role: &'static str, path: PathBuf },
    #[error(
        "byte offset {byte_offset} exceeds source length {source_len}{file_suffix}",
        file_suffix = format_file_suffix(.file.as_ref())
    )]
    OffsetOutOfBounds {
        file: Option<PathBuf>,
        byte_offset: usize,
        source_len: usize,
    },
    #[error(
        "byte offset {byte_offset} is not a UTF-8 character boundary{file_suffix}",
        file_suffix = format_file_suffix(.file.as_ref())
    )]
    OffsetNotCharBoundary {
        file: Option<PathBuf>,
        byte_offset: usize,
    },
    #[error(
        "UTF-16 offset {utf16_offset} for byte offset {byte_offset} exceeds Corsa's u32 limit{file_suffix}",
        file_suffix = format_file_suffix(.file.as_ref())
    )]
    OffsetOverflow {
        file: Option<PathBuf>,
        byte_offset: usize,
        utf16_offset: usize,
    },
    #[error("Corsa returned {returned} type results for {requested} requested offsets in `{file}`")]
    ResponseLengthMismatch {
        file: PathBuf,
        requested: usize,
        returned: usize,
    },
    #[error(
        "Corsa returned {returned} symbol results for {requested} definition offsets in `{file}`"
    )]
    DefinitionResponseLengthMismatch {
        file: PathBuf,
        requested: usize,
        returned: usize,
    },
    #[error("Corsa returned non-numeric {role} handle `{handle}`; expected an unsigned integer")]
    InvalidNumericHandle { role: &'static str, handle: String },
    #[error("failed to decode Corsa `{method}` response for `{file}`: {source}")]
    DecodeResponse {
        method: &'static str,
        file: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("Corsa diagnostic {code} for `{file}` returned invalid UTF-16 range {start}..{end}")]
    InvalidDiagnosticRange {
        file: String,
        code: i32,
        start: i64,
        end: i64,
    },
    #[error(
        "Corsa operation `{operation}` failed for `{file}` with config `{config}` and executable `{executable}`: {source}"
    )]
    Corsa {
        operation: &'static str,
        executable: PathBuf,
        config: PathBuf,
        file: PathBuf,
        #[source]
        source: Box<corsa::CorsaError>,
    },
}

impl CompilerTypeProviderError {
    fn with_file(self, file: PathBuf) -> Self {
        match self {
            Self::OffsetOutOfBounds {
                byte_offset,
                source_len,
                ..
            } => Self::OffsetOutOfBounds {
                file: Some(file),
                byte_offset,
                source_len,
            },
            Self::OffsetNotCharBoundary { byte_offset, .. } => Self::OffsetNotCharBoundary {
                file: Some(file),
                byte_offset,
            },
            Self::OffsetOverflow {
                byte_offset,
                utf16_offset,
                ..
            } => Self::OffsetOverflow {
                file: Some(file),
                byte_offset,
                utf16_offset,
            },
            other => other,
        }
    }
}

fn reject_diagnostic_suppression(
    source: &str,
    file_path: &std::path::Path,
) -> Result<(), CompilerTypeProviderError> {
    let allocator = Allocator::default();
    let name = file_path.to_str().unwrap_or("file.js");
    let parsed = parse(&allocator, name, source);
    let directive = parsed.program.comments.iter().find_map(|comment| {
        let comment_text = comment.content_span().source_text(source);
        let lowercase = comment_text.to_ascii_lowercase();
        ["@ts-nocheck", "@ts-expect-error", "@ts-ignore"]
            .into_iter()
            .find(|directive| lowercase.contains(directive))
    });

    if let Some(directive) = directive {
        Err(CompilerTypeProviderError::DiagnosticSuppression {
            path: file_path.to_path_buf(),
            directive,
        })
    } else {
        Ok(())
    }
}

fn reject_program_diagnostic_suppression(
    program_files: &[String],
    compiler_cwd: &std::path::Path,
) -> Result<(), CompilerTypeProviderError> {
    let source_paths = program_files
        .iter()
        .map(std::path::Path::new)
        .filter(|path| is_implementation_source(path))
        .map(|path| {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                compiler_cwd.join(path)
            };
            fs::canonicalize(&absolute).unwrap_or(absolute)
        })
        .collect::<BTreeSet<_>>();
    for path in source_paths {
        let source =
            fs::read_to_string(&path).map_err(|source| CompilerTypeProviderError::ReadSource {
                path: path.clone(),
                source,
            })?;
        reject_diagnostic_suppression(&source, &path)?;
    }
    Ok(())
}

fn is_implementation_source(path: &std::path::Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if file_name.ends_with(".d.ts")
        || file_name.ends_with(".d.mts")
        || file_name.ends_with(".d.cts")
    {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts"
            )
        })
}

fn validate_sound_compiler_options(
    options: &Value,
    request: &CompilerTypeRequest,
) -> Result<(), CompilerTypeProviderError> {
    let Some(options) = options.as_object() else {
        return Err(CompilerTypeProviderError::InvalidCompilerOptions {
            config: request.config_path.clone(),
            message: format!("expected an object, received {options}"),
        });
    };
    let boolean = |name: &'static str| {
        let Some(value) = options.get(name) else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Bool(value) => Ok(Some(*value)),
            value => Err(CompilerTypeProviderError::InvalidCompilerOptions {
                config: request.config_path.clone(),
                message: format!("`{name}` must be boolean, received {value}"),
            }),
        }
    };

    if boolean("noCheck")? == Some(true) {
        return Err(CompilerTypeProviderError::NoCheckEnabled {
            config: request.config_path.clone(),
        });
    }

    // In the pinned TypeScript 7 runtime, an unspecified strict flag defaults
    // to enabled. An explicit strictNullChecks value overrides `strict`.
    let strict = boolean("strict")?;
    let strict_null_checks = boolean("strictNullChecks")?;
    let strict_null_checks_enabled = strict_null_checks.unwrap_or(strict.unwrap_or(true));
    if !strict_null_checks_enabled {
        return Err(CompilerTypeProviderError::StrictNullChecksDisabled {
            config: request.config_path.clone(),
        });
    }

    if is_javascript_path(&request.file_path) && boolean("checkJs")? != Some(true) {
        return Err(CompilerTypeProviderError::CheckJsDisabled {
            config: request.config_path.clone(),
            file: request.file_path.clone(),
        });
    }

    Ok(())
}

fn is_javascript_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "jsx" | "mjs" | "cjs"
            )
        })
}

fn require_source_in_project(
    project_files: &[String],
    source_file: &std::path::Path,
    compiler_cwd: &std::path::Path,
    case_sensitive: bool,
    config_path: &std::path::Path,
) -> Result<(), CompilerTypeProviderError> {
    let expected = normalized_path_key(source_file, compiler_cwd, case_sensitive);
    let contains_source = project_files.iter().any(|file| {
        normalized_path_key(std::path::Path::new(file), compiler_cwd, case_sensitive) == expected
    });
    if contains_source {
        Ok(())
    } else {
        Err(CompilerTypeProviderError::SourceOutsideProject {
            config: config_path.to_path_buf(),
            file: source_file.to_path_buf(),
        })
    }
}

fn require_resolved_project_config(
    resolved_config: &str,
    requested_config: &std::path::Path,
    compiler_cwd: &std::path::Path,
    case_sensitive: bool,
) -> Result<(), CompilerTypeProviderError> {
    let resolved = std::path::Path::new(resolved_config);
    if normalized_path_key(resolved, compiler_cwd, case_sensitive)
        == normalized_path_key(requested_config, compiler_cwd, case_sensitive)
    {
        Ok(())
    } else {
        Err(CompilerTypeProviderError::ResolvedProjectMismatch {
            requested: requested_config.to_path_buf(),
            resolved: if resolved.is_absolute() {
                resolved.to_path_buf()
            } else {
                compiler_cwd.join(resolved)
            },
        })
    }
}

fn normalized_path_key(
    path: &std::path::Path,
    compiler_cwd: &std::path::Path,
    case_sensitive: bool,
) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        compiler_cwd.join(path)
    };
    let normalized = fs::canonicalize(&absolute).unwrap_or(absolute);
    let key = normalized.to_string_lossy().replace('\\', "/");
    if case_sensitive {
        key
    } else {
        key.to_lowercase()
    }
}

fn sort_and_deduplicate_diagnostics(diagnostics: &mut Vec<CompilerDiagnostic>) {
    diagnostics.sort();
    let mut seen = BTreeSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.file.clone(),
            diagnostic.severity,
            diagnostic.code.clone(),
            diagnostic.source.clone(),
            diagnostic.message.clone(),
            diagnostic.range,
        ))
    });
}

fn require_absolute(
    path: &std::path::Path,
    role: &'static str,
) -> Result<(), CompilerTypeProviderError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(CompilerTypeProviderError::RelativePath {
            role,
            path: path.to_path_buf(),
        })
    }
}

fn path_as_utf8(
    path: &std::path::Path,
    role: &'static str,
) -> Result<String, CompilerTypeProviderError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| CompilerTypeProviderError::NonUtf8Path {
            role,
            path: path.to_path_buf(),
        })
}

fn parse_numeric_handle(
    role: &'static str,
    handle: &str,
) -> Result<u64, CompilerTypeProviderError> {
    handle
        .parse::<u64>()
        .map_err(|_| CompilerTypeProviderError::InvalidNumericHandle {
            role,
            handle: handle.to_owned(),
        })
}

fn format_file_suffix(file: Option<&PathBuf>) -> String {
    file.map_or_else(String::new, |file| format!(" in `{}`", file.display()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorsaDiagnostic {
    #[serde(default)]
    file_name: String,
    pos: i64,
    end: i64,
    code: i32,
    category: i32,
    text: String,
    #[serde(default)]
    message_chain: Vec<CorsaDiagnostic>,
}

fn convert_diagnostic(
    diagnostic: CorsaDiagnostic,
    kind: CompilerDiagnosticKind,
    requested_file: &str,
) -> Result<CompilerDiagnostic, CompilerTypeProviderError> {
    let file = if diagnostic.file_name.is_empty() {
        requested_file.to_owned()
    } else {
        diagnostic.file_name.clone()
    };
    let (start_utf16, end_utf16) = if diagnostic.pos == -1 && diagnostic.end == -1 {
        (0, 0)
    } else {
        let start_utf16 = u32::try_from(diagnostic.pos).ok();
        let end_utf16 = u32::try_from(diagnostic.end).ok();
        let Some((start_utf16, end_utf16)) = start_utf16.zip(end_utf16) else {
            return Err(CompilerTypeProviderError::InvalidDiagnosticRange {
                file,
                code: diagnostic.code,
                start: diagnostic.pos,
                end: diagnostic.end,
            });
        };
        if end_utf16 < start_utf16 {
            return Err(CompilerTypeProviderError::InvalidDiagnosticRange {
                file,
                code: diagnostic.code,
                start: diagnostic.pos,
                end: diagnostic.end,
            });
        }
        (start_utf16, end_utf16)
    };

    let message = diagnostic_message(&diagnostic);
    Ok(CompilerDiagnostic {
        file,
        kind,
        severity: match diagnostic.category {
            0 => CompilerDiagnosticSeverity::Warning,
            1 => CompilerDiagnosticSeverity::Error,
            2 => CompilerDiagnosticSeverity::Hint,
            3 => CompilerDiagnosticSeverity::Information,
            _ => CompilerDiagnosticSeverity::Unspecified,
        },
        code: Some(diagnostic.code.to_string()),
        source: Some("typescript".to_owned()),
        message,
        range: CompilerRange {
            start_utf16,
            end_utf16,
        },
    })
}

fn diagnostic_message(diagnostic: &CorsaDiagnostic) -> String {
    let mut parts = Vec::new();
    collect_diagnostic_messages(diagnostic, &mut parts);
    parts.join(": ")
}

fn collect_diagnostic_messages(diagnostic: &CorsaDiagnostic, parts: &mut Vec<String>) {
    if !diagnostic.text.is_empty() {
        parts.push(diagnostic.text.clone());
    }
    for child in &diagnostic.message_chain {
        collect_diagnostic_messages(child, parts);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompilerDiagnostic, CompilerDiagnosticKind, CompilerDiagnosticSeverity, CompilerRange,
        CompilerTypeProviderError, CompilerTypeRequest, CorsaDiagnostic, CorsaTypeProvider,
        convert_diagnostic, definitions_are_declaration_backed, project_diagnostic_queries,
        reject_diagnostic_suppression, reject_program_diagnostic_suppression,
        require_resolved_project_config, require_source_in_project,
        sort_and_deduplicate_diagnostics, utf16_offset_for_byte_offset,
        validate_sound_compiler_options,
    };
    use std::{env, path::PathBuf};

    use serde_json::json;

    fn compiler_request(file: &str) -> CompilerTypeRequest {
        CompilerTypeRequest {
            config_path: PathBuf::from("/project/tsconfig.json"),
            file_path: PathBuf::from(file),
            source: String::new(),
            byte_offsets: Vec::new(),
            callable_byte_offsets: Vec::new(),
            definition_byte_offsets: Vec::new(),
        }
    }

    #[test]
    fn trusts_only_nonempty_declaration_file_definition_sets() {
        assert!(definitions_are_declaration_backed(&[
            "/typescript/lib/lib.es2025.d.ts".into(),
            "/types/runtime.d.mts".into(),
            "/types/legacy.d.cts".into(),
        ]));
        assert!(!definitions_are_declaration_backed(&[]));
        assert!(!definitions_are_declaration_backed(&[
            "/types/runtime.d.ts".into(),
            "/project/implementation.ts".into(),
        ]));
    }

    #[test]
    fn converts_utf8_byte_offsets_to_utf16_code_units() {
        let source = "aé🙂z";
        let cases = [(0, 0), (1, 1), (3, 2), (7, 4), (8, 5)];
        for (byte_offset, expected) in cases {
            assert_eq!(
                utf16_offset_for_byte_offset(source, byte_offset).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn rejects_offsets_inside_utf8_characters() {
        let error = utf16_offset_for_byte_offset("aé🙂z", 2).unwrap_err();
        assert!(matches!(
            error,
            CompilerTypeProviderError::OffsetNotCharBoundary { byte_offset: 2, .. }
        ));
    }

    #[test]
    fn rejects_offsets_past_the_source() {
        let error = utf16_offset_for_byte_offset("hello", 6).unwrap_err();
        assert!(matches!(
            error,
            CompilerTypeProviderError::OffsetOutOfBounds {
                byte_offset: 6,
                source_len: 5,
                ..
            }
        ));
    }

    #[test]
    fn rejects_typescript_diagnostic_suppression_in_comments() {
        let cases = [
            ("@ts-nocheck", "// @ts-nocheck\nconst value = 1;"),
            (
                "@ts-ignore",
                "const value = 1;\n/* @ts-ignore: deliberate */\nvalue();",
            ),
            (
                "@ts-expect-error",
                "const value = 1;\n/**\n * @ts-expect-error reason\n */\nvalue();",
            ),
        ];
        for (expected, source) in cases {
            let error = reject_diagnostic_suppression(source, std::path::Path::new("index.ts"))
                .unwrap_err();
            assert!(matches!(
                error,
                CompilerTypeProviderError::DiagnosticSuppression { directive, .. }
                    if directive == expected
            ));
        }
    }

    #[test]
    fn does_not_treat_javascript_literals_as_suppression_comments() {
        let source = r#"
            const line = "// @ts-ignore";
            const pattern = /@ts-nocheck/;
            const template = `/* @ts-expect-error */`;
        "#;
        reject_diagnostic_suppression(source, std::path::Path::new("index.js")).unwrap();
    }

    #[test]
    fn rejects_suppression_in_transitive_program_implementation_sources() {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rt/fixtures/compiler/project");
        let error = reject_program_diagnostic_suppression(
            &[
                "entry.js".to_string(),
                "missing-library.d.ts".to_string(),
                "dependency.js".to_string(),
            ],
            &project,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CompilerTypeProviderError::DiagnosticSuppression { path, directive }
                if path.ends_with("dependency.js") && directive == "@ts-nocheck"
        ));
    }

    #[test]
    fn rejects_unsound_normalized_compiler_options() {
        let ts_request = compiler_request("/project/index.ts");
        let js_request = compiler_request("/project/index.js");

        assert!(matches!(
            validate_sound_compiler_options(
                &json!({ "noCheck": true, "strict": true }),
                &ts_request
            ),
            Err(CompilerTypeProviderError::NoCheckEnabled { .. })
        ));
        assert!(matches!(
            validate_sound_compiler_options(&json!({ "strict": false }), &ts_request),
            Err(CompilerTypeProviderError::StrictNullChecksDisabled { .. })
        ));
        assert!(matches!(
            validate_sound_compiler_options(
                &json!({ "strict": true, "strictNullChecks": false }),
                &ts_request,
            ),
            Err(CompilerTypeProviderError::StrictNullChecksDisabled { .. })
        ));
        assert!(matches!(
            validate_sound_compiler_options(&json!({ "strict": true }), &js_request),
            Err(CompilerTypeProviderError::CheckJsDisabled { .. })
        ));
        assert!(matches!(
            validate_sound_compiler_options(&json!([]), &ts_request),
            Err(CompilerTypeProviderError::InvalidCompilerOptions { .. })
        ));
    }

    #[test]
    fn accepts_effective_strict_null_checks_and_checked_javascript() {
        let ts_request = compiler_request("/project/index.ts");
        let js_request = compiler_request("/project/index.cjs");

        validate_sound_compiler_options(&json!({}), &ts_request).unwrap();
        validate_sound_compiler_options(
            &json!({ "strict": false, "strictNullChecks": true }),
            &ts_request,
        )
        .unwrap();
        validate_sound_compiler_options(&json!({ "strict": true, "checkJs": true }), &js_request)
            .unwrap();
    }

    #[test]
    fn requires_the_source_to_belong_to_the_parsed_project() {
        let files = vec!["src/INDEX.js".to_owned()];
        let source = std::path::Path::new("/project/src/index.js");
        let cwd = std::path::Path::new("/project");
        let config = std::path::Path::new("/project/tsconfig.json");

        require_source_in_project(&files, source, cwd, false, config).unwrap();
        assert!(matches!(
            require_source_in_project(&files, source, cwd, true, config),
            Err(CompilerTypeProviderError::SourceOutsideProject { .. })
        ));
    }

    #[test]
    fn requires_corsa_to_resolve_the_requested_project_config() {
        let cwd = std::path::Path::new("/project");
        let requested = std::path::Path::new("/project/tsconfig.json");

        require_resolved_project_config("tsconfig.json", requested, cwd, true).unwrap();
        assert!(matches!(
            require_resolved_project_config("nested/tsconfig.json", requested, cwd, true),
            Err(CompilerTypeProviderError::ResolvedProjectMismatch { .. })
        ));
    }

    #[test]
    fn rejects_source_text_that_differs_from_the_compiler_file() {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let file_path = project.join("Cargo.toml");
        let provider = CorsaTypeProvider::new(env::current_exe().unwrap(), &project);
        let error = provider
            .analyze(&CompilerTypeRequest {
                config_path: file_path.clone(),
                file_path: file_path.clone(),
                source: "not the Cargo manifest".to_string(),
                byte_offsets: Vec::new(),
                callable_byte_offsets: Vec::new(),
                definition_byte_offsets: Vec::new(),
            })
            .unwrap_err();
        assert!(matches!(
            error,
            CompilerTypeProviderError::SourceMismatch { path } if path == file_path
        ));
    }

    #[test]
    fn converts_raw_diagnostics_to_owned_utf16_diagnostics() {
        let diagnostic = convert_diagnostic(
            CorsaDiagnostic {
                file_name: String::new(),
                pos: 3,
                end: 8,
                code: 2322,
                category: 1,
                text: "Type mismatch".to_owned(),
                message_chain: vec![CorsaDiagnostic {
                    file_name: String::new(),
                    pos: 0,
                    end: 0,
                    code: 2322,
                    category: 1,
                    text: "number is not assignable to string".to_owned(),
                    message_chain: Vec::new(),
                }],
            },
            CompilerDiagnosticKind::Semantic,
            "/project/index.ts",
        )
        .unwrap();

        assert_eq!(diagnostic.file, "/project/index.ts");
        assert_eq!(diagnostic.kind, CompilerDiagnosticKind::Semantic);
        assert_eq!(diagnostic.severity, CompilerDiagnosticSeverity::Error);
        assert_eq!(diagnostic.code.as_deref(), Some("2322"));
        assert_eq!(
            diagnostic.message,
            "Type mismatch: number is not assignable to string"
        );
        assert_eq!(
            diagnostic.range,
            CompilerRange {
                start_utf16: 3,
                end_utf16: 8,
            }
        );
    }

    #[test]
    fn project_file_diagnostic_queries_are_not_scoped_to_the_requested_source() {
        let queries = project_diagnostic_queries(7, "p/project", "/project/tsconfig.json");
        let file_methods = [
            "getSyntacticDiagnostics",
            "getBindDiagnostics",
            "getSemanticDiagnostics",
            "getSuggestionDiagnostics",
        ];

        let file_queries = queries
            .iter()
            .filter(|(method, ..)| file_methods.contains(method))
            .collect::<Vec<_>>();
        assert_eq!(file_queries.len(), file_methods.len());

        for (method, _, params, fallback_file) in file_queries {
            assert_eq!(params.get("snapshot"), Some(&json!(7)));
            assert_eq!(params.get("project"), Some(&json!("p/project")));
            assert!(
                params.get("files").is_none(),
                "{method} must omit `files` so Corsa checks the whole project"
            );
            assert_eq!(*fallback_file, "/project/tsconfig.json");
        }
    }

    #[test]
    fn preserves_the_file_name_of_a_foreign_project_diagnostic() {
        let diagnostic = convert_diagnostic(
            CorsaDiagnostic {
                file_name: "/project/dependency.ts".to_owned(),
                pos: 12,
                end: 18,
                code: 2322,
                category: 1,
                text: "Type 'string' is not assignable to type 'number'.".to_owned(),
                message_chain: Vec::new(),
            },
            CompilerDiagnosticKind::Semantic,
            "/project/tsconfig.json",
        )
        .unwrap();

        assert_eq!(diagnostic.file, "/project/dependency.ts");
        assert_eq!(diagnostic.severity, CompilerDiagnosticSeverity::Error);
    }

    #[test]
    fn converts_diagnostics_without_source_ranges_to_an_empty_sentinel() {
        let diagnostic = convert_diagnostic(
            CorsaDiagnostic {
                file_name: String::new(),
                pos: -1,
                end: -1,
                code: 2318,
                category: 1,
                text: "Cannot find global type".to_owned(),
                message_chain: Vec::new(),
            },
            CompilerDiagnosticKind::Global,
            "/project/tsconfig.json",
        )
        .unwrap();

        assert_eq!(diagnostic.file, "/project/tsconfig.json");
        assert_eq!(
            diagnostic.range,
            CompilerRange {
                start_utf16: 0,
                end_utf16: 0,
            }
        );
    }

    #[test]
    fn sorts_and_deduplicates_diagnostics() {
        let first = CompilerDiagnostic {
            file: "/project/a.ts".to_owned(),
            kind: CompilerDiagnosticKind::Config,
            severity: CompilerDiagnosticSeverity::Error,
            code: Some("1000".to_owned()),
            source: Some("typescript".to_owned()),
            message: "first".to_owned(),
            range: CompilerRange {
                start_utf16: 0,
                end_utf16: 0,
            },
        };
        let second = CompilerDiagnostic {
            file: "/project/z.ts".to_owned(),
            kind: CompilerDiagnosticKind::Semantic,
            severity: CompilerDiagnosticSeverity::Warning,
            code: Some("2000".to_owned()),
            source: Some("typescript".to_owned()),
            message: "second".to_owned(),
            range: CompilerRange {
                start_utf16: 4,
                end_utf16: 8,
            },
        };
        let mut duplicate_across_endpoint = first.clone();
        duplicate_across_endpoint.kind = CompilerDiagnosticKind::Global;
        let mut diagnostics = vec![
            second.clone(),
            duplicate_across_endpoint,
            first.clone(),
            second.clone(),
        ];

        sort_and_deduplicate_diagnostics(&mut diagnostics);

        assert_eq!(diagnostics, vec![first, second]);
    }

    #[test]
    fn real_corsa_provider_when_explicitly_configured() {
        let variables = [
            "REFINEJS_CORSA_TEST_BIN",
            "REFINEJS_CORSA_TEST_CWD",
            "REFINEJS_CORSA_TEST_CONFIG",
            "REFINEJS_CORSA_TEST_FILE",
            "REFINEJS_CORSA_TEST_OFFSET",
        ];
        let configured = variables
            .iter()
            .filter_map(|name| env::var(name).ok().map(|value| (*name, value)))
            .collect::<Vec<_>>();
        // The compiler-backed integration test uses only the executable
        // variable with repository fixtures. This lower-level provider probe
        // remains opt-in through its four additional project variables.
        if configured.is_empty()
            || (configured.len() == 1 && configured[0].0 == "REFINEJS_CORSA_TEST_BIN")
        {
            return;
        }
        assert_eq!(
            configured.len(),
            variables.len(),
            "real Corsa test requires all of: {}",
            variables.join(", ")
        );
        let value = |name: &str| {
            configured
                .iter()
                .find_map(|(key, value)| (*key == name).then_some(value.clone()))
                .unwrap()
        };
        let offset = value("REFINEJS_CORSA_TEST_OFFSET")
            .parse::<usize>()
            .expect("REFINEJS_CORSA_TEST_OFFSET must be a non-negative integer");
        let provider = CorsaTypeProvider::new(
            PathBuf::from(value("REFINEJS_CORSA_TEST_BIN")),
            PathBuf::from(value("REFINEJS_CORSA_TEST_CWD")),
        );
        let analysis = provider
            .analyze(&CompilerTypeRequest {
                config_path: PathBuf::from(value("REFINEJS_CORSA_TEST_CONFIG")),
                file_path: PathBuf::from(value("REFINEJS_CORSA_TEST_FILE")),
                source: std::fs::read_to_string(value("REFINEJS_CORSA_TEST_FILE"))
                    .expect("configured Corsa source must be readable"),
                byte_offsets: vec![offset],
                callable_byte_offsets: vec![offset],
                definition_byte_offsets: vec![offset],
            })
            .unwrap();
        assert_eq!(analysis.types.len(), 1);
        assert!(analysis.types[0].rendered_type.is_some());
        assert!(
            !analysis.types[0].call_return_types.is_empty(),
            "configured offset must point at a callable such as Array.map"
        );
        if let Ok(expected) = env::var("REFINEJS_CORSA_TEST_EXPECT_RETURN_TYPE") {
            assert!(
                analysis.types[0]
                    .call_return_types
                    .iter()
                    .any(|actual| actual == &expected),
                "expected call return type `{expected}`, got {:?}",
                analysis.types[0].call_return_types
            );
        }
        if env::var_os("REFINEJS_CORSA_TEST_EXPECT_DIAGNOSTIC").is_some() {
            assert!(
                !analysis.diagnostics.is_empty(),
                "configured source must contain a TypeScript diagnostic"
            );
        }
    }
}
