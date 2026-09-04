//! Ambient JavaScript library declarations and target-environment selection.
//!
//! User-authored `/*#rt */` annotations remain separate from this catalog.
//! The verifier looks up globals, members, and modules on [`LibraryRegistry`].

mod catalog;
mod environment;
mod model;

pub use environment::{
    Environment, EnvironmentError, EnvironmentEvidence, EvidenceKind, detect_environment,
    detect_environment_from_program,
};
pub use model::{
    CallbackTiming, CallbackUse, FunctionEffects, FunctionSignature, LibraryExport, LibraryModule,
    LibraryParameter, LibraryRegistry, ReceiverEffect, SemanticRefinement,
};

/// Resolve `Auto` from an already parsed program and build its deterministic
/// library catalog. An explicit environment wins over incidental source markers.
pub fn registry_for_program(
    requested: Environment,
    program: &pragma_parse::Program<'_>,
) -> Result<LibraryRegistry, EnvironmentError> {
    let environment = match requested {
        Environment::Auto => detect_environment_from_program(program)?,
        environment => environment,
    };
    Ok(catalog::build(environment))
}

#[cfg(test)]
mod tests;
