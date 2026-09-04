use pragma_parse::{parse, semantic_graph, unresolved_root_names, Allocator};

#[test]
fn js_and_ts_comments_are_on_the_program() {
    let js = "/* keep */\nconst x = 1;\n";
    let ts = "/* keep */\nconst x: number = 1;\n";
    for (name, src) in [("a.js", js), ("a.ts", ts)] {
        let allocator = Allocator::new();
        let parsed = parse(&allocator, name, src);
        assert!(
            parsed.diagnostics.is_empty(),
            "{name} diagnostics: {:?}",
            parsed.diagnostics
        );
        assert!(
            !parsed.program.comments.is_empty(),
            "{name} should keep comments"
        );
        let text: Vec<_> = parsed
            .program
            .comments
            .iter()
            .map(|c| c.content_span().source_text(src))
            .collect();
        assert!(
            text.iter().any(|t| t.contains("keep")),
            "{name} comments: {text:?}"
        );
    }
}

#[test]
fn semantic_graph_unresolved_includes_deno() {
    let src = "Deno.readFile('x');\n";
    let allocator = Allocator::new();
    let parsed = parse(&allocator, "detect.js", src);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let semantic = semantic_graph(&parsed.program);
    let names = unresolved_root_names(&semantic);
    assert!(
        names.iter().any(|n| n == "Deno"),
        "unresolved names: {names:?}"
    );
}
