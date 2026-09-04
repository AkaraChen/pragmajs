use pragma_rt::{checker, parser, prelude::Environment, syntax, transpiler};

const SQRT_SOURCE: &str = r#"
/*#rt
 * type: (n: number | n > 0) => number | $ > 0
 */
function sqrt(n) {
  return Math.sqrt(n);
}

/*#rt type: number | x > 0 */
const x = 9;
"#;

#[test]
fn parser_finds_annotations() {
    let result = parser::parse_file(SQRT_SOURCE, "test.js").unwrap();
    assert!(result.annotations.len() >= 3);
    let params: Vec<_> = result
        .annotations
        .iter()
        .filter(|a| matches!(a.target, syntax::AnnotationTarget::Param { .. }))
        .collect();
    assert!(!params.is_empty());
}

#[test]
fn parser_accepts_dotted_named_types_and_generics() {
    let source = r#"
/*#rt type: (server: Bun.Server, box: Vendor.Box<Bun.Server>) => Bun.Server */
function keepServer(server, box) {
  return server;
}
"#;
    let result = parser::parse_file(source, "dotted.js").unwrap();
    let parameter_types = result
        .annotations
        .iter()
        .filter_map(|annotation| match &annotation.target {
            syntax::AnnotationTarget::Param { index, .. } => Some((*index, &annotation.ty.base)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parameter_types,
        vec![
            (0, &syntax::BaseType::Named("Bun.Server".into())),
            (
                1,
                &syntax::BaseType::Generic(
                    "Vendor.Box".into(),
                    vec![syntax::BaseType::Named("Bun.Server".into())],
                ),
            ),
        ]
    );
    let return_type = result
        .annotations
        .iter()
        .find_map(|annotation| match &annotation.target {
            syntax::AnnotationTarget::Return { .. } => Some(&annotation.ty.base),
            _ => None,
        });
    assert_eq!(
        return_type,
        Some(&syntax::BaseType::Named("Bun.Server".into()))
    );
}

#[test]
fn checker_resolves_dotted_catalog_receiver_types() {
    let source = r#"
/*#rt type: (server: Bun.Server) => number | $ >= 0 */
function serverPort(server) {
  return server.port;
}
"#;
    let result = parser::parse_file(source, "dotted.js").unwrap();
    let errors = checker::check_source_with_environment(
        source,
        "dotted.js",
        &result.annotations,
        Environment::Bun,
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn checker_accepts_valid() {
    let result = parser::parse_file(SQRT_SOURCE, "test.js").unwrap();
    let errors = checker::check_source_with_environment(
        SQRT_SOURCE,
        "test.js",
        &result.annotations,
        Environment::Auto,
    );
    assert!(errors.is_empty(), "{:?}", errors);
}

#[test]
fn checker_rejects_unknown_identifier() {
    let source = "/*#rt type: number | y > 0 */\nconst x = 5;";
    let result = parser::parse_file(source, "test.js").unwrap();
    let errors = checker::check_source_with_environment(
        source,
        "test.js",
        &result.annotations,
        Environment::Auto,
    );
    assert!(!errors.is_empty());
}

#[test]
fn transpiler_injects_assertions() {
    let result = parser::parse_file(SQRT_SOURCE, "test.js").unwrap();
    let out = transpiler::transpile(SQRT_SOURCE, "test.js", &result.annotations).unwrap();
    assert!(out.contains("__rt.assert"));
    assert!(out.contains("Math.sqrt"));
}

const SQRT_TS_SOURCE: &str = r#"
/*#rt
 * type: (n: number | n > 0) => number | $ > 0
 */
function sqrt(n: number) {
  return Math.sqrt(n);
}

/*#rt type: number | x > 0 */
const x: number = 9;
"#;

#[test]
fn typescript_type_syntax_checks_and_transpiles() {
    let result = parser::parse_file(SQRT_TS_SOURCE, "test.ts").unwrap();
    let errors = checker::check_source_with_environment(
        SQRT_TS_SOURCE,
        "test.ts",
        &result.annotations,
        Environment::Auto,
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("JavaScript parse errors")),
        "{errors:?}"
    );
    assert!(errors.is_empty(), "{errors:?}");
    let out = transpiler::transpile(SQRT_TS_SOURCE, "test.ts", &result.annotations).unwrap();
    assert!(out.contains("__rt.assert"));
}
