use crate::syntax::{BaseType, PredicateExpr};

use super::{
    CallbackTiming, Environment, EnvironmentError, LibraryExport, ReceiverEffect,
    SemanticRefinement, catalog, detect_environment, registry_for_source,
};

fn detect(source: &str) -> Result<Environment, EnvironmentError> {
    detect_environment(source, "detect.js")
}

#[test]
fn auto_detection_uses_unbound_globals_and_module_sources() {
    assert_eq!(detect("const value = 1;").unwrap(), Environment::Ecmascript);
    assert_eq!(
        detect("import { readFile } from 'node:fs'; readFile('x', () => {});").unwrap(),
        Environment::Node
    );
    assert_eq!(
        detect("await import('bun:test');").unwrap(),
        Environment::Bun
    );
    assert_eq!(
        detect("Deno.readTextFile('x');").unwrap(),
        Environment::Deno
    );
    assert_eq!(
        detect("document.querySelector('main');").unwrap(),
        Environment::Browser
    );
}

#[test]
fn auto_detection_ignores_local_bindings_comments_and_strings() {
    let local = "const Deno = { readTextFile() {} }; Deno.readTextFile();";
    assert_eq!(detect(local).unwrap(), Environment::Ecmascript);
    let inert = "// Bun.file('x')\nconst text = 'document process node:fs';";
    assert_eq!(detect(inert).unwrap(), Environment::Ecmascript);
}

#[test]
fn node_compatibility_markers_do_not_conflict_with_deno_or_bun() {
    let deno = "import { join } from 'node:path'; Deno.readTextFile(join('a', 'b'));";
    assert_eq!(detect(deno).unwrap(), Environment::Deno);
    let bun = "import { readFileSync } from 'node:fs'; Bun.file('x');";
    assert_eq!(detect(bun).unwrap(), Environment::Bun);
}

#[test]
fn incompatible_runtime_markers_report_deterministic_evidence() {
    let error = detect("Deno.cwd(); Bun.file('x');").unwrap_err();
    let EnvironmentError::Conflict {
        candidates,
        evidence,
    } = &error
    else {
        panic!("expected conflict, got {error:?}");
    };
    assert_eq!(candidates, &[Environment::Deno, Environment::Bun]);
    assert_eq!(evidence.len(), 2);
    assert_eq!(
        error.to_string(),
        "conflicting environment markers for deno, bun; deno global 'Deno'; bun global 'Bun'"
    );

    let browser_node = detect("import 'node:fs'; document.body;").unwrap_err();
    assert!(matches!(
        browser_node,
        EnvironmentError::Conflict { ref candidates, .. }
            if candidates == &[Environment::Browser, Environment::Node]
    ));
}

#[test]
fn explicit_environment_wins_over_source_markers() {
    let selected = registry_for_source(Environment::Node, "Deno.cwd();", "detect.js").unwrap();
    assert_eq!(selected.environment(), Environment::Node);
}

#[test]
fn ecmascript_catalog_exposes_generic_array_contracts_and_refinements() {
    let catalog = catalog::build(Environment::Ecmascript);
    let map = &catalog.receiver_method("Array", "map").unwrap()[0];
    assert_eq!(map.effects.receiver, ReceiverEffect::Read);
    assert_eq!(map.effects.callbacks[0].timing, CallbackTiming::Immediate);
    assert!(matches!(
        map.returns.base,
        BaseType::Array(ref element) if **element == BaseType::Named("$U".into())
    ));
    assert!(
        map.refinements
            .contains(&SemanticRefinement::ResultLengthEqualsReceiver)
    );
    let flat_map = catalog.receiver_method("Array", "flatMap").unwrap();
    assert_eq!(flat_map.len(), 2);
    assert!(flat_map.iter().all(|overload| {
        matches!(
            overload.returns.base,
            BaseType::Array(ref element) if **element == BaseType::Named("$U".into())
        )
    }));
    assert!(matches!(
        catalog
            .receiver_property("Array", "length")
            .unwrap()
            .ty
            .predicate,
        Some(PredicateExpr::Binary(..))
    ));
    let reduce = catalog.receiver_method("Array", "reduce").unwrap();
    assert_eq!(reduce.len(), 2);
    assert!(reduce.iter().any(|overload| overload.parameters.len() == 1));
    assert!(reduce.iter().any(|overload| overload.parameters.len() == 2));

    for path in ["Math.sqrt", "Math.abs", "Array.isArray", "Number.isInteger"] {
        assert!(
            catalog.static_function(path).is_some(),
            "catalog must expose {path} without annotation injection"
        );
    }
}

#[test]
fn runtime_catalogs_are_isolated_and_modules_support_aliases_and_overloads() {
    let browser = catalog::build(Environment::Browser);
    assert!(browser.global("document").is_some());
    assert!(
        browser
            .receiver_method("Document", "querySelector")
            .is_some()
    );
    assert!(browser.module("node:fs").is_none());

    for (receiver, method) in [
        ("Node", "appendChild"),
        ("HTMLElement", "click"),
        ("EventTarget", "dispatchEvent"),
    ] {
        assert!(
            browser.receiver_method(receiver, method).unwrap()[0]
                .effects
                .executes_user_code,
            "{receiver}.{method} must invalidate state that user code can mutate"
        );
    }
    assert_eq!(
        browser
            .receiver_method("EventTarget", "dispatchEvent")
            .unwrap()[0]
            .returns
            .base,
        BaseType::Primitive("boolean".into())
    );

    let node = catalog::build(Environment::Node);
    assert!(node.global("process").is_some());
    assert!(node.global("document").is_none());
    let canonical = node.module_export("node:fs", "readFileSync").unwrap();
    let alias = node.module_export("fs", "readFileSync").unwrap();
    assert_eq!(canonical, alias);
    assert!(matches!(canonical, LibraryExport::Function(overloads) if overloads.len() == 2));
    assert!(matches!(
        node.module_export("node:fs/promises", "readFile"),
        Some(LibraryExport::Function(overloads)) if overloads.len() == 2
    ));

    let deno = catalog::build(Environment::Deno);
    assert!(deno.global("Deno").is_some());
    assert!(deno.static_function("Deno.readTextFile").is_some());
    assert!(deno.module_export("node:path", "join").is_some());
    assert!(deno.global("process").is_some());
    assert!(deno.global("Buffer").is_some());
    assert!(deno.static_function("setTimeout").is_some());
    assert!(deno.static_function("clearTimeout").is_some());

    let bun = catalog::build(Environment::Bun);
    assert!(bun.global("Bun").is_some());
    assert!(bun.global("process").is_some());
    assert!(bun.static_function("Bun.serve").is_some());
    assert!(bun.receiver_property("Bun.Server", "port").is_some());
    assert!(bun.receiver_method("Bun.Server", "stop").is_some());
    assert!(
        bun.static_function("Bun.serve").unwrap()[0]
            .effects
            .executes_user_code
    );
    assert_eq!(
        bun.static_function("Bun.serve").unwrap()[0].parameters[0]
            .ty
            .base,
        BaseType::Primitive("object".into())
    );
    assert!(bun.module_export("bun:test", "test").is_some());
    assert!(bun.module_export("node:fs", "readFileSync").is_some());
}

#[test]
fn catalog_iteration_is_lexically_deterministic() {
    let catalog = catalog::build(Environment::Bun);
    let globals: Vec<_> = catalog.globals().map(|(name, _)| name).collect();
    assert!(globals.windows(2).all(|pair| pair[0] < pair[1]));
    let modules: Vec<_> = catalog.modules().map(|(name, _)| name).collect();
    assert!(modules.windows(2).all(|pair| pair[0] < pair[1]));
}
