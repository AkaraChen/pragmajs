//! Ambient JavaScript library declarations and target-environment selection.
//!
//! User-authored `/*#rt */` annotations remain separate from this catalog.
//! The verifier looks up globals, members, and modules on [`LibraryRegistry`].

mod catalog;
mod environment;
mod model;

pub use environment::{
    Environment, EnvironmentError, EnvironmentEvidence, EvidenceKind, detect_environment,
};
pub use model::{
    CallbackTiming, CallbackUse, FunctionEffects, FunctionSignature, LibraryExport, LibraryModule,
    LibraryParameter, LibraryRegistry, PropertySignature, ReceiverEffect, SemanticRefinement,
};

/// Resolve `Auto` from the source and build its deterministic library catalog.
/// An explicit environment wins over incidental source markers.
pub fn registry_for_source(
    requested: Environment,
    source: &str,
) -> Result<LibraryRegistry, EnvironmentError> {
    let environment = match requested {
        Environment::Auto => detect_environment(source)?,
        environment => environment,
    };
    Ok(catalog::build(environment))
}

#[cfg(test)]
mod tests;
