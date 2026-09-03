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
    let out = transpiler::transpile(SQRT_SOURCE, &result.annotations).unwrap();
    assert!(out.contains("__rt.assert"));
    assert!(out.contains("Math.sqrt"));
}
