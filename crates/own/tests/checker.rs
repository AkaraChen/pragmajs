//! Tests drive the shipped `check_source` / `check_paths` library APIs.
//! No mock checker. The unified CLI uses `check_program` on a shared parse.

use std::collections::HashMap;
use std::path::PathBuf;

use pragma_own::{check_paths, check_source, check_source_with, RuleKind, Runtime};

fn kinds(src: &str) -> Vec<RuleKind> {
    check_source("test.js", src).kinds()
}

fn has(src: &str, kind: RuleKind) -> bool {
    kinds(src).contains(&kind)
}

const PRELUDE: &str = r#"
/*#own type: () => unique Buffer */
function make() { return { bytes: 0 }; }
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: &readonly Buffer) => void */
function read(buf) {}
/*#own type: (buf: &mut Buffer) => void */
function write(buf) {}
"#;

fn with_process(body: &str) -> String {
    format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer) => void */
function process(buf) {{
{body}
}}
"
    )
}

#[test]
fn comments_attach_and_unique_forget() {
    let src = with_process("  // forgot buf\n");
    let result = check_source("test.js", &src);
    assert!(
        result.kinds().contains(&RuleKind::UniqueForget),
        "expected unique-forget (proves /*#own attached to the function); got {:?}",
        result.formatted_lines()
    );
}

#[test]
fn unique_double_move() {
    let src = format!(
        "{PRELUDE}
/*#own type: (a: unique Buffer, b: unique Buffer) => void */
function pair(a, b) {{ void a; void b; }}
/*#own type: (buf: unique Buffer) => void */
function process(buf) {{
  pair(buf, buf);
}}
"
    );
    assert!(
        has(&src, RuleKind::DoubleMove),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn unique_use_after_move() {
    let src = with_process("  consume(buf);\n  void buf;\n");
    assert!(
        has(&src, RuleKind::UseAfterMove),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn unique_consume_ok() {
    let src = with_process("  consume(buf);\n");
    let ks = kinds(&src);
    assert!(
        !ks.iter().any(|k| matches!(
            k,
            RuleKind::UniqueForget
                | RuleKind::DoubleMove
                | RuleKind::UseAfterMove
                | RuleKind::ConsumeWhileBorrowed
                | RuleKind::MutBorrowConflict
                | RuleKind::BorrowAfterMove
                | RuleKind::BorrowEscape
        )),
        "unexpected {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn affine_drop_ok() {
    let src = r#"
/*#own type: (f: affine File) => void */
function process(f) {}
"#;
    assert!(
        !has(src, RuleKind::UniqueForget),
        "got {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn affine_use_after_move() {
    let src = r#"
/*#own type: (f: affine File) => void */
function closeFile(f) { void f; }
/*#own type: (f: affine File) => void */
function process(f) {
  closeFile(f);
  void f;
}
"#;
    assert!(
        has(src, RuleKind::UseAfterMove),
        "got {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn readonly_then_consume_ok() {
    let src = with_process("  read(buf);\n  consume(buf);\n");
    let lines = check_source("test.js", &src).formatted_lines();
    assert!(
        !lines.iter().any(|l| l.contains("error[")
            && !l.contains("unmapped")
            && !l.contains("annot-parse")),
        "unexpected {lines:?}"
    );
}

#[test]
fn consume_while_borrowed() {
    let src = with_process(
        r#"
  /*#own borrow buf as view: &readonly Buffer */
  const view = buf;
  consume(buf);
  void view;
"#,
    );
    assert!(
        has(&src, RuleKind::ConsumeWhileBorrowed),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn mut_borrow_conflict_same_expression() {
    let src = format!(
        "{PRELUDE}
/*#own type: (a: &mut Buffer, b: &mut Buffer) => void */
function both(a, b) {{}}
/*#own type: (buf: unique Buffer) => void */
function process(buf) {{
  both(/*#own &mut */ buf, /*#own &mut */ buf);
  consume(buf);
}}
"
    );
    assert!(
        has(&src, RuleKind::MutBorrowConflict),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn readonly_vs_mut_conflict() {
    let src = format!(
        "{PRELUDE}
/*#own type: (r: &readonly Buffer, w: &mut Buffer) => void */
function mix(r, w) {{}}
/*#own type: (buf: unique Buffer) => void */
function process(buf) {{
  mix(/*#own &readonly */ buf, /*#own &mut */ buf);
  consume(buf);
}}
"
    );
    assert!(
        has(&src, RuleKind::MutBorrowConflict),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn borrow_after_move() {
    let src = with_process("  consume(buf);\n  read(buf);\n");
    assert!(
        has(&src, RuleKind::BorrowAfterMove),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn borrow_escape_via_return() {
    let src = format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer) => unique Buffer */
function process(buf) {{
  /*#own borrow buf as view: &readonly Buffer */
  const view = buf;
  return view;
}}
"
    );
    assert!(
        has(&src, RuleKind::BorrowEscape),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn copy_reuse_ok() {
    let src = r#"
/*#own type: (n: copy number) => copy number */
function twice(n) {
  return n + n;
}
"#;
    assert!(
        check_source("test.js", src).diagnostics.is_empty(),
        "got {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn clone_reuse_ok() {
    let src = with_process(
        r#"
  /*#own clone buf as copy */
  const copy = buf;
  consume(buf);
  consume(copy);
"#,
    );
    let lines = check_source("test.js", &src).formatted_lines();
    assert!(
        !lines.iter().any(|l| l.contains("use-after-move")
            || l.contains("double-move")
            || l.contains("unique-forget")),
        "unexpected {lines:?}"
    );
}

#[test]
fn branch_inconsistent() {
    let src = format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {{
  if (flag) {{
    consume(buf);
  }}
}}
"
    );
    assert!(
        has(&src, RuleKind::BranchInconsistent),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn consume_in_loop() {
    let src = with_process("  while (true) { consume(buf); }\n");
    assert!(
        has(&src, RuleKind::ConsumeInLoop),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn typescript_unique_forget() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function process(buf: unknown) {}
"#;
    let result = check_source("test.ts", src);
    assert!(
        result.kinds().contains(&RuleKind::UniqueForget),
        "got {:?}",
        result.formatted_lines()
    );
}

#[test]
fn borrow_lifetime_then_consume_ok() {
    let src = with_process(
        r#"
  {
    /*#own borrow buf as view: &readonly Buffer */
    const view = buf;
    read(view);
  }
  consume(buf);
"#,
    );
    let lines = check_source("test.js", &src).formatted_lines();
    assert!(
        !lines.iter().any(|l| l.contains("consume-while-borrowed")
            || l.contains("unique-forget")
            || l.contains("use-after-move")),
        "unexpected {lines:?}"
    );
}

#[test]
fn unmapped_eval() {
    let src = with_process("  eval('x');\n  consume(buf);\n");
    assert!(
        has(&src, RuleKind::UnmappedConstruct),
        "got {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

/// Directory walk on the real `examples/` tree via the library `check_paths` API.
#[test]
fn examples_directory_via_shipped_check_paths() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let result = check_paths(&[dir.clone()]).expect("check_paths");
    let mut by_file: HashMap<String, Vec<RuleKind>> = HashMap::new();
    for d in &result.diagnostics {
        let name = std::path::Path::new(&d.path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&d.path)
            .to_string();
        by_file.entry(name).or_default().push(d.kind);
    }

    let expected_err: &[(&str, RuleKind)] = &[
        ("err-unique-forget.js", RuleKind::UniqueForget),
        ("err-unique-double-move.js", RuleKind::DoubleMove),
        ("err-unique-use-after-move.js", RuleKind::UseAfterMove),
        ("err-affine-use-after-move.js", RuleKind::UseAfterMove),
        ("err-borrow-after-move.js", RuleKind::BorrowAfterMove),
        ("err-borrow-escape.js", RuleKind::BorrowEscape),
        ("err-branch-inconsistent.js", RuleKind::BranchInconsistent),
        ("err-consume-in-loop.js", RuleKind::ConsumeInLoop),
        (
            "err-consume-while-borrowed.js",
            RuleKind::ConsumeWhileBorrowed,
        ),
        ("err-overlapping-mut.js", RuleKind::MutBorrowConflict),
        ("err-readonly-mut-conflict.js", RuleKind::MutBorrowConflict),
        ("err-prelude-buffer-forget.js", RuleKind::UniqueForget),
    ];

    for (file, kind) in expected_err {
        let got = by_file.get(*file).cloned().unwrap_or_default();
        assert!(
            got.contains(kind),
            "{file} should report {kind:?}, got {got:?}; all={}",
            result.formatted_lines().join("\n")
        );
    }

    let ok_files = [
        "ok-unique-move.js",
        "ok-affine-drop.js",
        "ok-readonly-borrow.js",
        "ok-mut-borrow.js",
        "ok-lifetime-scope.js",
        "ok-copy.js",
        "ok-copy.ts",
        "ok-clone.js",
        "ok-branch-consume.js",
        "ok-create-in-loop.js",
        "ok-prelude-console.js",
        "ok-prelude-buffer-tostring.js",
        "ok-prelude-handle-close.js",
    ];
    for file in ok_files {
        let got = by_file.get(file).cloned().unwrap_or_default();
        assert!(
            got.is_empty(),
            "{file} should be clean, got {got:?}; all={}",
            result.formatted_lines().join("\n")
        );
    }

    assert!(
        dir.join("ok-copy.ts").is_file(),
        "TypeScript copy example must exist"
    );
}

#[test]
fn node_prelude_copy_does_not_consume() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  console.log(buf, "x");
  consume(buf);
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.diagnostics.is_empty(),
        "default node should not consume via console.log: {:?}",
        node.formatted_lines()
    );
    let none = check_source_with("test.js", src, Runtime::None);
    assert!(
        none.kinds().contains(&RuleKind::UseAfterMove),
        "prelude none should consume unknown callee args: {:?}",
        none.formatted_lines()
    );
}

#[test]
fn node_prelude_readonly_does_not_consume() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (a: unique Buffer, b: unique Buffer) => void */
function process(a, b) {
  Buffer.compare(a, b);
  consume(a);
  consume(b);
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.diagnostics.is_empty(),
        "Buffer.compare is &readonly: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn node_prelude_unique_param_moves() {
    let src = r#"
/*#own type: (fd: unique Fd) => void */
function process(fd) {
  fs.closeSync(fd);
  void fd;
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.kinds().contains(&RuleKind::UseAfterMove),
        "fs.closeSync should move unique Fd: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn none_does_not_apply_prelude_return() {
    let src = r#"
/*#own type: () => void */
function main() {
  const buf = Buffer.from("x");
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.kinds().contains(&RuleKind::UniqueForget),
        "node Buffer.from returns unique: {:?}",
        node.formatted_lines()
    );
    let none = check_source_with("test.js", src, Runtime::None);
    assert!(
        !none.kinds().contains(&RuleKind::UniqueForget),
        "none must not treat Buffer.from as annotated: {:?}",
        none.formatted_lines()
    );
}

#[test]
fn bun_only_name_resolves_under_bun() {
    let src = r#"
/*#own type: () => void */
function main() {
  const f = Bun.file("x");
}
"#;
    let bun = check_source_with("test.js", src, Runtime::Bun);
    assert!(
        bun.kinds().contains(&RuleKind::UniqueForget),
        "Bun.file returns unique under bun: {:?}",
        bun.formatted_lines()
    );
    let node = check_source_with("test.js", src, Runtime::Node);
    assert!(
        !node.kinds().contains(&RuleKind::UniqueForget),
        "Bun.file is absent from node: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn deno_only_name_resolves_under_deno() {
    let src = r#"
/*#own type: () => void */
function main() {
  const f = Deno.readFile("x");
}
"#;
    let deno = check_source_with("test.js", src, Runtime::Deno);
    assert!(
        deno.kinds().contains(&RuleKind::UniqueForget),
        "Deno.readFile returns unique under deno: {:?}",
        deno.formatted_lines()
    );
    let node = check_source_with("test.js", src, Runtime::Node);
    assert!(
        !node.kinds().contains(&RuleKind::UniqueForget),
        "Deno.readFile is absent from node: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn instance_method_readonly_does_not_consume() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  buf.toString();
  consume(buf);
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.diagnostics.is_empty(),
        "Buffer#toString is &readonly this: {:?}",
        node.formatted_lines()
    );
    let none = check_source_with("test.js", src, Runtime::None);
    assert!(
        none.diagnostics.is_empty(),
        "unknown instance calls are path reads, still not a prelude hit: {:?}",
        none.formatted_lines()
    );
}

#[test]
fn instance_method_unique_this_moves() {
    let src = r#"
/*#own type: (fh: unique FileHandle) => void */
function process(fh) {
  fh.close();
  void fh;
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.kinds().contains(&RuleKind::UseAfterMove),
        "FileHandle#close should move: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn bun_instance_method_only_under_bun() {
    let src = r#"
/*#own type: () => void */
function main() {
  const f = Bun.file("x");
  f.text();
}
"#;
    let bun = check_source_with("test.js", src, Runtime::Bun);
    assert!(
        bun.kinds().contains(&RuleKind::UniqueForget),
        "BunFile stays unique after #text: {:?}",
        bun.formatted_lines()
    );
    let node = check_source_with("test.js", src, Runtime::Node);
    assert!(
        !node.kinds().contains(&RuleKind::UniqueForget)
            || node.formatted_lines().iter().any(|l| l.contains("use-after-move")),
        "Bun.file is not a node prelude unique-return: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn deno_instance_method_only_under_deno() {
    let src = r#"
/*#own type: (f: unique FsFile) => void */
function process(f) {
  f.close();
}
"#;
    let deno = check_source_with("test.js", src, Runtime::Deno);
    assert!(
        !deno.kinds().contains(&RuleKind::UniqueForget),
        "FsFile#close consumes under deno: {:?}",
        deno.formatted_lines()
    );
    let node = check_source_with("test.js", src, Runtime::Node);
    assert!(
        node.kinds().contains(&RuleKind::UniqueForget),
        "node has no FsFile#close so close is a path read: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn child_process_spawn_hits_prelude() {
    let src = r#"
/*#own type: () => void */
function main() {
  const p = child_process.spawn("x");
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.kinds().contains(&RuleKind::UniqueForget),
        "child_process.spawn (underscore) must hit the node prelude: {:?}",
        node.formatted_lines()
    );
    let none = check_source_with("test.js", src, Runtime::None);
    assert!(
        !none.kinds().contains(&RuleKind::UniqueForget),
        "none must not treat child_process.spawn as annotated: {:?}",
        none.formatted_lines()
    );
}

#[test]
fn fs_alias_and_dotted_callee() {
    let src = r#"
/*#own type: () => void */
function main() {
  const a = fs.readFile("x");
  const b = readFile("y");
}
"#;
    let node = check_source("test.js", src);
    let forgets = node
        .diagnostics
        .iter()
        .filter(|d| d.kind == RuleKind::UniqueForget)
        .count();
    assert_eq!(
        forgets, 2,
        "both fs.readFile and readFile alias return unique: {:?}",
        node.formatted_lines()
    );
}

fn forgets_unique(src: &str) -> bool {
    check_source("test.js", src)
        .kinds()
        .contains(&RuleKind::UniqueForget)
}

fn forgets_unique_ts(src: &str) -> bool {
    check_source("test.ts", src)
        .kinds()
        .contains(&RuleKind::UniqueForget)
}

#[test]
fn top_level_unique_binding_is_forgotten() {
    let src = r#"
/*#own let buf: unique Buffer */
const buf = { bytes: 0 };
"#;
    assert!(
        forgets_unique(src),
        "top-level unique binding must unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn top_level_discarded_buffer_from_is_forgotten() {
    let src = r#"Buffer.from("x");"#;
    assert!(
        forgets_unique(src),
        "discarded Buffer.from at program scope: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn await_paren_and_as_keep_unique_return() {
    let src_await = r#"
/*#own type: () => void */
async function main() {
  const buf = await Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_await),
        "await Buffer.from must bind unique: {:?}",
        check_source("test.js", src_await).formatted_lines()
    );

    let src_paren = r#"
/*#own type: () => void */
function main() {
  const buf = (Buffer.from("x"));
}
"#;
    assert!(
        forgets_unique(src_paren),
        "parenthesized Buffer.from must bind unique: {:?}",
        check_source("test.js", src_paren).formatted_lines()
    );

    let src_as = r#"
/*#own type: () => void */
function main() {
  const buf = Buffer.from("x") as any;
}
"#;
    assert!(
        forgets_unique_ts(src_as),
        "TS `as` wrapper must bind unique: {:?}",
        check_source("test.ts", src_as).formatted_lines()
    );
}

#[test]
fn void_and_comma_discard_unique_return() {
    let src_void = r#"
/*#own type: () => void */
function main() {
  void Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_void),
        "void Buffer.from discards unique: {:?}",
        check_source("test.js", src_void).formatted_lines()
    );

    let src_comma = r#"
/*#own type: () => void */
function main() {
  (Buffer.from("x"), 1);
}
"#;
    assert!(
        forgets_unique(src_comma),
        "comma expression discards unique: {:?}",
        check_source("test.js", src_comma).formatted_lines()
    );
}

#[test]
fn anonymous_unique_passed_to_copy_callee_is_forgotten() {
    let src = r#"
/*#own type: () => void */
function main() {
  console.log(Buffer.from("x"));
}
"#;
    assert!(
        forgets_unique(src),
        "console.log(Buffer.from) must unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn unannotated_outer_still_checks_inner_own_let() {
    let src = r#"
function outer() {
  /*#own let buf: unique Buffer */
  const buf = { x: 1 };
}
"#;
    assert!(
        forgets_unique(src),
        "/*#own let inside unannotated function: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn unannotated_outer_still_checks_nested_annotated_function() {
    let src = r#"
function outer() {
  /*#own type: (buf: unique Buffer) => void */
  function process(buf) {}
}
"#;
    assert!(
        forgets_unique(src),
        "nested annotated function inside unannotated outer: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn class_and_object_method_unique_forget() {
    let src_class = r#"
class C {
  /*#own type: (buf: unique Buffer) => void */
  process(buf) {}
}
"#;
    assert!(
        forgets_unique(src_class),
        "class method unique-forget: {:?}",
        check_source("test.js", src_class).formatted_lines()
    );

    let src_obj = r#"
const o = {
  /*#own type: (buf: unique Buffer) => void */
  process(buf) {}
};
"#;
    assert!(
        forgets_unique(src_obj),
        "object-literal method unique-forget: {:?}",
        check_source("test.js", src_obj).formatted_lines()
    );
}

#[test]
fn try_finally_consume_is_use_after_move() {
    let src = with_process(
        r#"
  try {
    consume(buf);
  } finally {
    consume(buf);
  }
"#,
    );
    assert!(
        has(&src, RuleKind::UseAfterMove),
        "try consume then finally consume: {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn optional_member_is_path_not_consume() {
    let src = with_process("  buf?.x;\n  consume(buf);\n");
    let result = check_source("test.js", &src);
    assert!(
        !result.kinds().contains(&RuleKind::UseAfterMove)
            && !result.kinds().contains(&RuleKind::UniqueForget),
        "buf?.x is a path: {:?}",
        result.formatted_lines()
    );
}

#[test]
fn logical_and_does_not_silently_consume() {
    let src = format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {{
  flag && consume(buf);
}}
"
    );
    let ks = kinds(&src);
    assert!(
        ks.contains(&RuleKind::BranchInconsistent) || ks.contains(&RuleKind::UniqueForget),
        "flag && consume(buf) must not silently consume: {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn ternary_both_consume_is_not_double_move() {
    let src = format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {{
  flag ? consume(buf) : consume(buf);
}}
"
    );
    let result = check_source("test.js", &src);
    assert!(
        !result.kinds().contains(&RuleKind::DoubleMove),
        "ternary both-consume is one consume per path: {:?}",
        result.formatted_lines()
    );
    assert!(
        !result.kinds().contains(&RuleKind::UniqueForget),
        "ternary both-consume should consume: {:?}",
        result.formatted_lines()
    );
}

#[test]
fn borrow_alias_passed_to_unique_param_errors_on_owner() {
    let src = with_process(
        r#"
  /*#own borrow buf as view: &readonly Buffer */
  const view = buf;
  consume(view);
"#,
    );
    let result = check_source("test.js", &src);
    assert!(
        result.kinds().contains(&RuleKind::ConsumeWhileBorrowed)
            || result
                .formatted_lines()
                .iter()
                .any(|l| l.contains("`buf`")),
        "consume(view) must error on owner buf: {:?}",
        result.formatted_lines()
    );
}

#[test]
fn expression_arrow_and_callback_capture_not_silent() {
    let src_arrow = with_process(
        r#"
  const f = () => buf;
  consume(buf);
"#,
    );
    assert!(
        has(&src_arrow, RuleKind::UnmappedConstruct),
        "expression-bodied arrow capture: {:?}",
        check_source("test.js", &src_arrow).formatted_lines()
    );

    let src_cb = with_process(
        r#"
  setTimeout(() => consume(buf), 0);
  consume(buf);
"#,
    );
    assert!(
        has(&src_cb, RuleKind::UnmappedConstruct),
        "callback capture: {:?}",
        check_source("test.js", &src_cb).formatted_lines()
    );
}

#[test]
fn while_test_consume_is_consume_in_loop() {
    let src = with_process("  while (consume(buf)) {}\n");
    assert!(
        has(&src, RuleKind::ConsumeInLoop),
        "while test consume: {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn fs_read_does_not_move_fd() {
    let src = r#"
/*#own type: (fd: unique Fd) => void */
function process(fd) {
  fs.read(fd, buf, 0, 1, 0, function () {});
  fs.closeSync(fd);
}
"#;
    let node = check_source("test.js", src);
    assert!(
        !node.kinds().contains(&RuleKind::UseAfterMove),
        "fs.read must not move fd: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn fluent_and_slice_as_statement_do_not_unique_forget() {
    let src_on = r#"
/*#own type: (a: unique Agent) => void */
function consume(a) { void a; }
/*#own type: (a: unique Agent) => void */
function process(a) {
  a.addListener("x", function () {});
  consume(a);
}
"#;
    let node = check_source("test.js", src_on);
    assert!(
        !node.kinds().contains(&RuleKind::UniqueForget),
        "addListener as statement must not unique-forget: {:?}",
        node.formatted_lines()
    );

    let src_slice = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  buf.slice(0);
  consume(buf);
}
"#;
    let slice = check_source("test.js", src_slice);
    assert!(
        slice.diagnostics.is_empty(),
        "Buffer#slice as statement: {:?}",
        slice.formatted_lines()
    );
}

#[test]
fn fs_readfile_callback_statement_is_clean() {
    let src = r#"
/*#own type: () => void */
function main() {
  fs.readFile("x", {}, function () {});
}
"#;
    let node = check_source("test.js", src);
    assert!(
        !node.kinds().contains(&RuleKind::UniqueForget),
        "callback-form fs.readFile as statement: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn process_stdout_write_does_not_consume_buffer() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  process.stdout.write(buf);
  consume(buf);
}
"#;
    let node = check_source("test.js", src);
    assert!(
        node.diagnostics.is_empty(),
        "process.stdout.write then consume: {:?}",
        node.formatted_lines()
    );
}

#[test]
fn try_return_finally_consume_is_cleanup() {
    let src = with_process(
        r#"
  try {
    return;
  } finally {
    consume(buf);
  }
"#,
    );
    let result = check_source("test.js", &src);
    assert!(
        !result.kinds().contains(&RuleKind::UniqueForget)
            && !result.kinds().contains(&RuleKind::UseAfterMove),
        "try return; finally consume is cleanup: {:?}",
        result.formatted_lines()
    );
}

#[test]
fn optional_call_and_logical_unique_return() {
    let src_opt = r#"
/*#own type: () => void */
function main() {
  Buffer.from?.("x");
}
"#;
    assert!(
        forgets_unique(src_opt),
        "optional Buffer.from must unique-forget: {:?}",
        check_source("test.js", src_opt).formatted_lines()
    );

    let src_and = r#"
/*#own type: () => void */
function main() {
  flag && Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_and),
        "flag && Buffer.from must unique-forget: {:?}",
        check_source("test.js", src_and).formatted_lines()
    );

    let src_tern = r#"
/*#own type: () => void */
function main() {
  console.log(flag ? Buffer.from("x") : 1);
}
"#;
    assert!(
        forgets_unique(src_tern),
        "unique inside ?: passed to copy callee: {:?}",
        check_source("test.js", src_tern).formatted_lines()
    );
}

#[test]
fn capture_in_object_template_and_for() {
    let src_obj = with_process(
        r#"
  const f = () => ({ x: buf });
  consume(buf);
"#,
    );
    assert!(
        has(&src_obj, RuleKind::UnmappedConstruct),
        "object-literal capture: {:?}",
        check_source("test.js", &src_obj).formatted_lines()
    );

    let src_tpl = with_process(
        r#"
  const f = () => `${buf}`;
  consume(buf);
"#,
    );
    assert!(
        has(&src_tpl, RuleKind::UnmappedConstruct),
        "template capture: {:?}",
        check_source("test.js", &src_tpl).formatted_lines()
    );

    let src_for = with_process(
        r#"
  setTimeout(() => { for (;;) consume(buf); }, 0);
  consume(buf);
"#,
    );
    assert!(
        has(&src_for, RuleKind::UnmappedConstruct),
        "for-loop in callback capture: {:?}",
        check_source("test.js", &src_for).formatted_lines()
    );
}

#[test]
fn call_position_function_own_let_and_annotated() {
    let src_iife = r#"
(function () {
  /*#own let buf: unique Buffer */
  const buf = {};
})();
"#;
    assert!(
        forgets_unique(src_iife),
        "IIFE /*#own let: {:?}",
        check_source("test.js", src_iife).formatted_lines()
    );

    let src_map = r#"
[1].map(function () {
  /*#own let buf: unique Buffer */
  const buf = {};
});
"#;
    assert!(
        forgets_unique(src_map),
        "map callback /*#own let: {:?}",
        check_source("test.js", src_map).formatted_lines()
    );

    let src_to = r#"
setTimeout(/*#own type: (buf: unique Buffer) => void */ function process(buf) {}, 0);
"#;
    assert!(
        forgets_unique(src_to),
        "call-position annotated function: {:?}",
        check_source("test.js", src_to).formatted_lines()
    );
}

#[test]
fn unique_rvalue_in_return_bind_and_condition() {
    let src_ret = r#"
/*#own type: (flag: copy boolean) => void */
function process(flag) {
  return flag && Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_ret),
        "return flag && Buffer.from: {:?}",
        check_source("test.js", src_ret).formatted_lines()
    );

    let src_bind = r#"
/*#own type: () => void */
function main() {
  const x = flag && Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_bind),
        "const x = flag && Buffer.from: {:?}",
        check_source("test.js", src_bind).formatted_lines()
    );

    let src_if = r#"
/*#own type: () => void */
function main() {
  if (Buffer.from("x")) {}
}
"#;
    assert!(
        forgets_unique(src_if),
        "if (Buffer.from): {:?}",
        check_source("test.js", src_if).formatted_lines()
    );
}

#[test]
fn unique_arg_void_comma_and_ternary_test() {
    let src_void = r#"
/*#own type: () => void */
function main() {
  console.log(void Buffer.from("x"));
}
"#;
    assert!(
        forgets_unique(src_void),
        "console.log(void Buffer.from): {:?}",
        check_source("test.js", src_void).formatted_lines()
    );

    let src_comma = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: () => void */
function main() {
  consume((Buffer.from("x"), Buffer.from("y")));
}
"#;
    let r = check_source("test.js", src_comma);
    assert!(
        r.kinds().iter().filter(|k| **k == RuleKind::UniqueForget).count() >= 1,
        "comma unique intermediates: {:?}",
        r.formatted_lines()
    );

    let src_test = r#"
/*#own type: () => void */
function main() {
  console.log(Buffer.from("x") ? 1 : 2);
}
"#;
    assert!(
        forgets_unique(src_test),
        "ternary test unique: {:?}",
        check_source("test.js", src_test).formatted_lines()
    );
}

#[test]
fn exclusive_logical_through_ts_as() {
    let src = r#"
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {
  consume((flag && buf) as any);
}
"#;
    let r = check_source("test.ts", src);
    assert!(
        r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::UniqueForget),
        "consume((flag && buf) as any): {:?}",
        r.formatted_lines()
    );
}

#[test]
fn nested_arrow_inside_callback_captures() {
    let src = with_process(
        r#"
  setTimeout(() => { const g = () => buf; }, 0);
  consume(buf);
"#,
    );
    assert!(
        has(&src, RuleKind::UnmappedConstruct),
        "nested arrow in callback: {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn expression_bodied_arrow_discards_unique_return() {
    let src = r#"
const f = () => Buffer.from("x");
"#;
    assert!(
        forgets_unique(src),
        "() => Buffer.from must unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn optional_call_does_not_definite_consume() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  consume?.(buf);
  consume(buf);
}
"#;
    let r = check_source("test.js", src);
    assert!(
        !r.kinds().contains(&RuleKind::UseAfterMove),
        "optional consume then consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn for_init_update_and_switch_case_test_unique_forget() {
    let src_for = r#"
/*#own type: () => void */
function main() {
  for (Buffer.from("x"); false; ) {}
}
"#;
    assert!(
        forgets_unique(src_for),
        "for init Buffer.from: {:?}",
        check_source("test.js", src_for).formatted_lines()
    );

    let src_case = r#"
/*#own type: () => void */
function main() {
  switch (1) {
    case Buffer.from("x"):
      break;
  }
}
"#;
    assert!(
        forgets_unique(src_case),
        "switch case Buffer.from: {:?}",
        check_source("test.js", src_case).formatted_lines()
    );
}

#[test]
fn ts_as_wrapped_arrow_capture_and_throw_unique() {
    let src_as = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  const f = (() => buf) as any;
  consume(buf);
}
"#;
    let r = check_source("test.ts", src_as);
    assert!(
        r.kinds().contains(&RuleKind::UnmappedConstruct),
        "(() => buf) as any capture: {:?}",
        r.formatted_lines()
    );

    let src_throw = r#"
/*#own type: () => unique Buffer */
function process() {
  throw Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_throw),
        "throw unique in unique-returning fn: {:?}",
        check_source("test.js", src_throw).formatted_lines()
    );
}

#[test]
fn discarded_create_server_still_unique_forgets() {
    let src = r#"
/*#own type: () => void */
function main() {
  http.createServer(function () {});
}
"#;
    assert!(
        forgets_unique(src),
        "createServer(fn) must unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn empty_switch_case_test_unique_forget() {
    let src = r#"
/*#own type: () => void */
function main() {
  switch (1) {
    case Buffer.from("x"):
    case 2:
      break;
  }
}
"#;
    assert!(
        forgets_unique(src),
        "empty case Buffer.from: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn unique_return_sequence_and_ternary() {
    let src_seq = r#"
/*#own type: () => unique Buffer */
function process() {
  return (Buffer.from("x"), Buffer.from("y"));
}
"#;
    assert!(
        forgets_unique(src_seq),
        "return (from x, from y) leaks x: {:?}",
        check_source("test.js", src_seq).formatted_lines()
    );

    let src_tern = r#"
/*#own type: (flag: copy boolean) => unique Buffer */
function process(flag) {
  return flag ? Buffer.from("x") : Buffer.from("y");
}
"#;
    let r = check_source("test.js", src_tern);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "return unique ?: unique should transfer: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn try_early_return_finally_empty_does_not_leak() {
    let src = format!(
        "{PRELUDE}
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {{
  try {{
    if (flag) return;
    consume(buf);
  }} finally {{}}
}}
"
    );
    let r = check_source("test.js", &src);
    assert!(
        r.kinds().contains(&RuleKind::UniqueForget)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::UseAfterMove),
        "early return then consume in try with empty finally: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn try_finally_then_consume_is_clean() {
    let src = with_process(
        r#"
  try {
  } finally {
  }
  consume(buf);
"#,
    );
    let r = check_source("test.js", &src);
    assert!(
        r.diagnostics.is_empty(),
        "empty try/finally then consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn sequence_and_ternary_const_unique_bind() {
    let src_seq = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: () => void */
function main() {
  const x = (Buffer.from("x"), Buffer.from("y"));
  consume(x);
}
"#;
    let r = check_source("test.js", src_seq);
    assert!(
        r.kinds().contains(&RuleKind::UniqueForget),
        "sequence const leaks non-last unique: {:?}",
        r.formatted_lines()
    );
    assert!(
        !r.kinds().contains(&RuleKind::UseAfterMove),
        "last unique is bound and consumed: {:?}",
        r.formatted_lines()
    );

    let src_tern = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (flag: copy boolean) => void */
function process(flag) {
  const x = flag ? Buffer.from("x") : Buffer.from("y");
  consume(x);
}
"#;
    let t = check_source("test.js", src_tern);
    assert!(
        t.diagnostics.is_empty(),
        "ternary unique bind then consume: {:?}",
        t.formatted_lines()
    );
}

#[test]
fn unique_producer_as_method_receiver_is_forgotten() {
    let src = r#"
/*#own type: () => void */
function main() {
  Buffer.from("x").toString();
}
"#;
    assert!(
        forgets_unique(src),
        "Buffer.from().toString must unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn logical_and_drops_left_unique() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: () => void */
function main() {
  const x = Buffer.from("x") && Buffer.from("y");
  consume(x);
}
"#;
    let r = check_source("test.js", src);
    assert!(
        r.kinds().contains(&RuleKind::UniqueForget),
        "&& drops left unique: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn nested_try_finally_outer_cleanup() {
    let src = with_process(
        r#"
  try {
    try {
      return;
    } finally {
    }
  } finally {
    consume(buf);
  }
"#,
    );
    let r = check_source("test.js", &src);
    assert!(
        !r.kinds().contains(&RuleKind::UseAfterMove),
        "outer finally consume after inner return: {:?}",
        r.formatted_lines()
    );
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "outer finally should consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn nested_and_in_unique_bind_drops_inner_left() {
    let src = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (flag: copy boolean) => void */
function process(flag) {
  const x = flag && (Buffer.from("x") && Buffer.from("y"));
  consume(x);
}
"#;
    let r = check_source("test.js", src);
    assert!(
        r.kinds().contains(&RuleKind::UniqueForget),
        "nested && left unique in bind: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn nested_finally_double_consume_is_use_after_move() {
    let src = with_process(
        r#"
  try {
    try {
      return;
    } finally {
      consume(buf);
    }
  } finally {
    consume(buf);
  }
"#,
    );
    let r = check_source("test.js", &src);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove),
        "inner and outer finally both consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn catch_return_finally_empty_unique_forget() {
    let src = with_process(
        r#"
  try {
    return;
  } catch (e) {
    return;
  } finally {
  }
"#,
    );
    assert!(
        has(&src, RuleKind::UniqueForget),
        "try/catch both return empty finally: {:?}",
        check_source("test.js", &src).formatted_lines()
    );
}

#[test]
fn unique_in_array_object_unary_binary_template() {
    let src = r#"
/*#own type: () => void */
function main() {
  [Buffer.from("x")];
}
"#;
    assert!(
        forgets_unique(src),
        "[Buffer.from] unique-forget: {:?}",
        check_source("test.js", src).formatted_lines()
    );
}

#[test]
fn class_field_and_spread_and_computed_key_unique_forget() {
    let src_field = r#"
class C {
  x = Buffer.from("x");
}
"#;
    assert!(
        forgets_unique(src_field),
        "class field Buffer.from: {:?}",
        check_source("test.js", src_field).formatted_lines()
    );

    let src_spread = r#"
/*#own type: () => void */
function main() {
  [...[Buffer.from("x")]];
}
"#;
    assert!(
        forgets_unique(src_spread),
        "spread Buffer.from: {:?}",
        check_source("test.js", src_spread).formatted_lines()
    );

    let src_key = r#"
/*#own type: () => void */
function main() {
  ({ [Buffer.from("x")]: 1 });
}
"#;
    assert!(
        forgets_unique(src_key),
        "computed key Buffer.from: {:?}",
        check_source("test.js", src_key).formatted_lines()
    );
}

#[test]
fn call_spread_and_unannotated_default_unique_forget() {
    let src_spread = r#"
/*#own type: () => void */
function main() {
  console.log(...[Buffer.from("x")]);
}
"#;
    assert!(
        forgets_unique(src_spread),
        "console.log(...[Buffer.from]): {:?}",
        check_source("test.js", src_spread).formatted_lines()
    );

    let src_def = r#"
function main(x = Buffer.from("x")) {}
"#;
    assert!(
        forgets_unique(src_def),
        "unannotated default Buffer.from: {:?}",
        check_source("test.js", src_def).formatted_lines()
    );

    let src_export = r#"
export default Buffer.from("x");
"#;
    assert!(
        forgets_unique(src_export),
        "export default Buffer.from: {:?}",
        check_source("test.js", src_export).formatted_lines()
    );
}

#[test]
fn nested_copy_callee_and_pattern_default_unique_forget() {
    let src_nested = r#"
/*#own type: () => void */
function main() {
  console.log(console.log(Buffer.from("x")));
}
"#;
    assert!(
        forgets_unique(src_nested),
        "nested console.log(Buffer.from): {:?}",
        check_source("test.js", src_nested).formatted_lines()
    );

    let src_pat = r#"
/*#own type: () => void */
function main() {
  const { x = Buffer.from("x") } = {};
}
"#;
    assert!(
        forgets_unique(src_pat),
        "pattern default Buffer.from: {:?}",
        check_source("test.js", src_pat).formatted_lines()
    );
}

#[test]
fn assignment_pattern_and_update_lhs_unique_forget() {
    let src_assign = r#"
/*#own type: () => void */
function main() {
  ({ x = Buffer.from("x") } = {});
}
"#;
    assert!(
        forgets_unique(src_assign),
        "assign pattern default: {:?}",
        check_source("test.js", src_assign).formatted_lines()
    );

    let src_upd = r#"
/*#own type: () => void */
function main() {
  Buffer.from("x").y = 1;
}
"#;
    assert!(
        forgets_unique(src_upd),
        "unique receiver assignment: {:?}",
        check_source("test.js", src_upd).formatted_lines()
    );

    let src_param = r#"
function main({ x = Buffer.from("x") }) {}
"#;
    assert!(
        forgets_unique(src_param),
        "unannotated param pattern default: {:?}",
        check_source("test.js", src_param).formatted_lines()
    );
}

#[test]
fn nested_assign_and_rest_param_unique_forget() {
    let src_nested = r#"
/*#own type: () => void */
function main() {
  ({ a: { x: y = Buffer.from("x") } } = {});
}
"#;
    assert!(
        forgets_unique(src_nested),
        "nested assign pattern: {:?}",
        check_source("test.js", src_nested).formatted_lines()
    );

    let src_rest = r#"
function main(...[x = Buffer.from("x")]) {}
"#;
    assert!(
        forgets_unique(src_rest),
        "rest param pattern: {:?}",
        check_source("test.js", src_rest).formatted_lines()
    );
}

#[test]
fn nested_assign_array_rest_and_template_iife_unique_forget() {
    let src_rest = r#"
/*#own type: () => void */
function main() {
  ({ a: [...[x = Buffer.from("x")]] } = {});
}
"#;
    assert!(
        forgets_unique(src_rest),
        "nested assign array rest: {:?}",
        check_source("test.js", src_rest).formatted_lines()
    );

    let src_tpl = r#"
/*#own type: () => void */
function main() {
  `${function () { Buffer.from("x"); }}`;
}
"#;
    assert!(
        forgets_unique(src_tpl),
        "template IIFE unique: {:?}",
        check_source("test.js", src_tpl).formatted_lines()
    );
}

#[test]
fn parenthesized_fn_object_class_unique_forget() {
    let src_fn = r#"
/*#own type: () => void */
function main() {
  (function () { Buffer.from("x"); });
}
"#;
    assert!(
        forgets_unique(src_fn),
        "parenthesized function: {:?}",
        check_source("test.js", src_fn).formatted_lines()
    );

    let src_arrow = r#"
/*#own type: () => void */
function main() {
  (() => Buffer.from("x"));
}
"#;
    assert!(
        forgets_unique(src_arrow),
        "parenthesized arrow: {:?}",
        check_source("test.js", src_arrow).formatted_lines()
    );

    let src_and = r#"
/*#own type: () => void */
function main() {
  true && (() => Buffer.from("x"));
}
"#;
    assert!(
        forgets_unique(src_and),
        "logical && arrow: {:?}",
        check_source("test.js", src_and).formatted_lines()
    );

    let src_obj = r#"
/*#own type: () => void */
function main() {
  ({ m() { Buffer.from("x"); } });
}
"#;
    assert!(
        forgets_unique(src_obj),
        "parenthesized object method: {:?}",
        check_source("test.js", src_obj).formatted_lines()
    );

    let src_class = r#"
/*#own type: () => void */
function main() {
  (class { m() { Buffer.from("x"); } });
}
"#;
    assert!(
        forgets_unique(src_class),
        "parenthesized class method: {:?}",
        check_source("test.js", src_class).formatted_lines()
    );

    let src_ann = r#"
/*#own type: () => void */
function main() {
  ({ /*#own type: (buf: unique Buffer) => void */ process(buf) {} });
}
"#;
    assert!(
        forgets_unique(src_ann),
        "parenthesized annotated object method: {:?}",
        check_source("test.js", src_ann).formatted_lines()
    );
}

#[test]
fn discard_nested_fn_in_defaults_unique_forget() {
    let src_rest = r#"
/*#own type: () => void */
function main() {
  ({ a: [...[x = function () { Buffer.from("x"); }]] } = {});
}
"#;
    assert!(
        forgets_unique(src_rest),
        "assign rest default nested function: {:?}",
        check_source("test.js", src_rest).formatted_lines()
    );

    let src_pat = r#"
const { x = () => Buffer.from("x") } = {};
"#;
    assert!(
        forgets_unique(src_pat),
        "pattern default nested arrow: {:?}",
        check_source("test.js", src_pat).formatted_lines()
    );

    let src_param = r#"
function main(x = () => Buffer.from("x")) {}
"#;
    assert!(
        forgets_unique(src_param),
        "param default nested arrow: {:?}",
        check_source("test.js", src_param).formatted_lines()
    );

    let src_key = r#"
class C {
  [function () { Buffer.from("x"); }]() {}
}
"#;
    assert!(
        forgets_unique(src_key),
        "computed method key nested function: {:?}",
        check_source("test.js", src_key).formatted_lines()
    );
}

#[test]
fn annotated_const_arrow_return_is_not_discard() {
    let src = r#"
/*#own type: () => unique Buffer */
const f = () => Buffer.from("x");
"#;
    let r = check_source("test.js", src);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "unique-returning const arrow must not discard its body: {:?}",
        r.formatted_lines()
    );
}

fn forgets_unique_named(filename: &str, src: &str) -> bool {
    check_source(filename, src)
        .kinds()
        .contains(&RuleKind::UniqueForget)
}

#[test]
fn for_in_of_lhs_and_ts_assignment_unique_forget() {
    let src_of = r#"
/*#own type: () => void */
function main() {
  const obj = {};
  for (obj[Buffer.from("x")] of []) {}
}
"#;
    assert!(
        forgets_unique(src_of),
        "for-of computed lhs: {:?}",
        check_source("test.js", src_of).formatted_lines()
    );

    let src_in = r#"
/*#own type: () => void */
function main() {
  const obj = {};
  for (obj[Buffer.from("x")] in {}) {}
}
"#;
    assert!(
        forgets_unique(src_in),
        "for-in computed lhs: {:?}",
        check_source("test.js", src_in).formatted_lines()
    );

    let src_as = r#"
/*#own type: () => void */
function main() {
  const obj = {};
  (obj[Buffer.from("x")] as any) = 1;
}
"#;
    assert!(
        forgets_unique_named("test.ts", src_as),
        "TS as assignment lhs: {:?}",
        check_source("test.ts", src_as).formatted_lines()
    );
}

#[test]
fn private_in_decorator_enum_namespace_export_eq_unique_forget() {
    let src_priv = r#"
class C {
  #x = 1;
  m() { #x in Buffer.from("x"); }
}
"#;
    assert!(
        forgets_unique(src_priv),
        "private-in Buffer.from: {:?}",
        check_source("test.js", src_priv).formatted_lines()
    );

    let src_dec = r#"
@(() => Buffer.from("x"))
class C {}
"#;
    assert!(
        forgets_unique_named("test.ts", src_dec),
        "class decorator: {:?}",
        check_source("test.ts", src_dec).formatted_lines()
    );

    let src_param_dec = r#"
function f(@(() => Buffer.from("x")) x) {}
"#;
    assert!(
        forgets_unique_named("test.ts", src_param_dec),
        "param decorator: {:?}",
        check_source("test.ts", src_param_dec).formatted_lines()
    );

    let src_enum = r#"
enum E { A = Buffer.from("x") as any }
"#;
    assert!(
        forgets_unique_named("test.ts", src_enum),
        "enum member: {:?}",
        check_source("test.ts", src_enum).formatted_lines()
    );

    let src_ns = r#"
namespace N { Buffer.from("x"); }
"#;
    assert!(
        forgets_unique_named("test.ts", src_ns),
        "namespace body: {:?}",
        check_source("test.ts", src_ns).formatted_lines()
    );

    let src_eq = r#"
export = Buffer.from("x");
"#;
    assert!(
        forgets_unique_named("test.ts", src_eq),
        "export = : {:?}",
        check_source("test.ts", src_eq).formatted_lines()
    );
}

#[test]
fn jsx_and_annotated_class_field_unique() {
    let src_jsx = r#"
const x = <div>{Buffer.from("x")}</div>;
"#;
    assert!(
        forgets_unique_named("test.tsx", src_jsx),
        "jsx child: {:?}",
        check_source("test.tsx", src_jsx).formatted_lines()
    );

    let src_attr = r#"
const x = <div data={Buffer.from("x")} />;
"#;
    assert!(
        forgets_unique_named("test.tsx", src_attr),
        "jsx attr: {:?}",
        check_source("test.tsx", src_attr).formatted_lines()
    );

    let src_field = r#"
class C {
  /*#own type: () => unique Buffer */
  f = () => Buffer.from("x");
}
"#;
    let r = check_source("test.js", src_field);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "annotated unique-returning class field must not discard: {:?}",
        r.formatted_lines()
    );

    let src_pat = r#"
const { x = { /*#own type: () => unique Buffer */ m() { return Buffer.from("x"); } } } = {};
"#;
    let r = check_source("test.js", src_pat);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "annotated method in pattern-default object must not discard: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn ts_update_with_and_computed_type_keys_unique_forget() {
    let src_upd = r#"
/*#own type: () => void */
function main() {
  const obj: any = {};
  ++(obj[Buffer.from("x")] as any);
}
"#;
    assert!(
        forgets_unique_named("test.ts", src_upd),
        "TS-wrapped ++ lhs: {:?}",
        check_source("test.ts", src_upd).formatted_lines()
    );

    let src_with = r#"
with (Buffer.from("x")) {}
"#;
    assert!(
        forgets_unique(src_with),
        "with object Buffer.from: {:?}",
        check_source("test.js", src_with).formatted_lines()
    );

    let src_iface = r#"
interface I { [Buffer.from("x")]: number }
"#;
    assert!(
        forgets_unique_named("test.ts", src_iface),
        "interface computed key: {:?}",
        check_source("test.ts", src_iface).formatted_lines()
    );

    let src_alias = r#"
type T = { [Buffer.from("x")]: number };
"#;
    assert!(
        forgets_unique_named("test.ts", src_alias),
        "type alias computed key: {:?}",
        check_source("test.ts", src_alias).formatted_lines()
    );

    let src_ann = r#"
const y: { [Buffer.from("x")]: number } = {};
"#;
    assert!(
        forgets_unique_named("test.ts", src_ann),
        "var type annotation computed key: {:?}",
        check_source("test.ts", src_ann).formatted_lines()
    );
}

#[test]
fn callee_forms_and_nested_ts_types_unique_forget() {
    for src in [
        r#"(0, Buffer.from)("x");"#,
        r#"Buffer["from"]("x");"#,
        r#"globalThis.Buffer.from("x");"#,
        r#"new Buffer.from("x");"#,
    ] {
        assert!(
            forgets_unique(src),
            "callee form {src}: {:?}",
            check_source("test.js", src).formatted_lines()
        );
    }

    let src_as = r#"
const y = 1 as { [Buffer.from("x")]: number };
"#;
    assert!(
        forgets_unique_named("test.ts", src_as),
        "as type computed key: {:?}",
        check_source("test.ts", src_as).formatted_lines()
    );

    let src_ref = r#"
type T = Array<{ [Buffer.from("x")]: number }>;
"#;
    assert!(
        forgets_unique_named("test.ts", src_ref),
        "type ref args: {:?}",
        check_source("test.ts", src_ref).formatted_lines()
    );

    let src_mapped = r#"
type T = { [K in string]: { [Buffer.from("x")]: number } };
"#;
    assert!(
        forgets_unique_named("test.ts", src_mapped),
        "mapped type: {:?}",
        check_source("test.ts", src_mapped).formatted_lines()
    );

    let src_this = r#"
function f(this: { [Buffer.from("x")]: number }) {}
"#;
    assert!(
        forgets_unique_named("test.ts", src_this),
        "this param type: {:?}",
        check_source("test.ts", src_this).formatted_lines()
    );
}

#[test]
fn new_constructor_args_and_unique_param_default() {
    let src_copy = r#"
/*#own type: (n: copy any) => unique Foo */
function Foo(n) { return n; }
/*#own type: (f: unique Foo) => void */
function consumeFoo(f) { void f; }
const x = new Foo(Buffer.from("x"));
consumeFoo(x);
"#;
    assert!(
        forgets_unique(src_copy),
        "new Foo(Buffer.from) copy param: {:?}",
        check_source("test.js", src_copy).formatted_lines()
    );

    let src_unique = r#"
/*#own type: (buf: unique Buffer) => unique Foo */
function Foo(buf) { return buf; }
/*#own type: (f: unique Foo) => void */
function consumeFoo(f) { void f; }
const x = new Foo(Buffer.from("x"));
consumeFoo(x);
"#;
    let r = check_source("test.js", src_unique);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "new Foo unique param consumes Buffer.from: {:?}",
        r.formatted_lines()
    );

    let src_def = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf = Buffer.from("x")) { consume(buf); }
"#;
    let r = check_source("test.js", src_def);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "unique param default transfer: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn hash_copy_import_type_and_prototype_fluent() {
    let src_copy = r#"
/*#own type: (h: unique Hash) => void */
function process(h) {
  const h2 = h.copy();
}
"#;
    assert!(
        forgets_unique(src_copy),
        "Hash#copy produces unique: {:?}",
        check_source("test.js", src_copy).formatted_lines()
    );

    let src_imp = r#"
type T = import("fs", { assert: { type: Buffer.from("x") } });
"#;
    assert!(
        forgets_unique_named("test.ts", src_imp),
        "import() type options: {:?}",
        check_source("test.ts", src_imp).formatted_lines()
    );

    let src_proto = r#"
/*#own type: (a: unique Agent) => void */
function consume(a) { void a; }
/*#own type: (a: unique Agent) => void */
function process(a) {
  Agent.prototype.on("x", function () {});
  consume(a);
}
"#;
    let r = check_source("test.js", src_proto);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "Agent.prototype.on fluent copy: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn exclusive_in_template_spread_and_try_catch_return() {
    let src_tpl = with_process(
        r#"
  `${flag && buf}`;
  consume(buf);
"#,
    );
    let r = check_source("test.js", &src_tpl);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::UniqueForget),
        "template flag && buf then consume: {:?}",
        r.formatted_lines()
    );

    let src_try = with_process(
        r#"
  try {
    if (flag) return;
    consume(buf);
  } catch {
    consume(buf);
  } finally {}
"#,
    );
    assert!(
        has(&src_try, RuleKind::UniqueForget) || has(&src_try, RuleKind::BranchInconsistent),
        "try return then consume / catch consume empty finally: {:?}",
        check_source("test.js", &src_try).formatted_lines()
    );
}

#[test]
fn tagged_template_instance_copy_and_jsx_ident_once() {
    let src_tag = r#"
/*#own type: (s: copy any) => unique Buffer */
function uniqueTag(s) { return Buffer.from(s); }
uniqueTag`hello`;
"#;
    assert!(
        forgets_unique(src_tag),
        "tagged uniqueTag: {:?}",
        check_source("test.js", src_tag).formatted_lines()
    );

    let src_copy = r#"
/*#own type: () => copy Hash */
function getHash() { return {}; }
getHash().copy();
"#;
    assert!(
        forgets_unique(src_copy),
        "getHash().copy unique: {:?}",
        check_source("test.js", src_copy).formatted_lines()
    );

    let src_jsx = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  const x = <div>{buf}</div>;
}
"#;
    let r = check_source("test.tsx", src_jsx);
    let uam = r
        .diagnostics
        .iter()
        .filter(|d| d.kind == RuleKind::UseAfterMove)
        .count();
    assert_eq!(
        uam, 0,
        "jsx {{buf}} should not double-consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn bound_copy_receiver_nullish_and_jsx_capture() {
    let src_recv = r#"
const s = Buffer.from("x").toString();
"#;
    assert!(
        forgets_unique(src_recv),
        "bound toString unique receiver: {:?}",
        check_source("test.js", src_recv).formatted_lines()
    );

    let src_nullish = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: () => void */
function main() {
  consume(null ?? Buffer.from("x"));
}
"#;
    let r = check_source("test.js", src_nullish);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "null ?? Buffer.from should transfer: {:?}",
        r.formatted_lines()
    );

    let src_cap = with_process(
        r#"
  setTimeout(() => { const x = <div>{buf}</div>; }, 0);
  consume(buf);
"#,
    );
    assert!(
        check_source("test.tsx", &src_cap)
            .kinds()
            .contains(&RuleKind::UnmappedConstruct),
        "jsx capture in callback: {:?}",
        check_source("test.tsx", &src_cap).formatted_lines()
    );
}

#[test]
fn unique_this_tagged_interp_and_finally_double_consume() {
    let src_close = r#"
fs.promises.open("x").close();
"#;
    let r = check_source("test.js", src_close);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "open().close() consumes unique this: {:?}",
        r.formatted_lines()
    );

    let src_tag = r#"
/*#own type: (s: copy any, buf: unique Buffer) => void */
function take(s, buf) { void buf; }
take`${Buffer.from("x")}`;
"#;
    let r = check_source("test.js", src_tag);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "tagged interp unique param: {:?}",
        r.formatted_lines()
    );

    let src_fin = with_process(
        r#"
  try {
    if (flag) return;
    consume(buf);
  } finally {
    consume(buf);
  }
"#,
    );
    assert!(
        has(&src_fin, RuleKind::UseAfterMove) || has(&src_fin, RuleKind::UniqueForget),
        "try consume then finally consume: {:?}",
        check_source("test.js", &src_fin).formatted_lines()
    );
}

#[test]
fn buffer_filter_unique_and_await_ident_move() {
    let src_f = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  buf.filter(() => true);
  consume(buf);
}
"#;
    assert!(
        forgets_unique(src_f),
        "Buffer#filter unique result: {:?}",
        check_source("test.js", src_f).formatted_lines()
    );

    let src_await = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
async function process(buf) {
  const x = await buf;
  consume(x);
}
"#;
    let r = check_source("test.js", src_await);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "await buf should move: {:?}",
        r.formatted_lines()
    );
    assert!(
        !r.kinds().contains(&RuleKind::UseAfterMove),
        "await buf move then consume x: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn nested_annotated_callee_and_nested_jsx_exclusive() {
    let src_ns = r#"
namespace N {
  /*#own type: () => unique Buffer */
  function inner() { return Buffer.from("x"); }
  inner();
}
"#;
    assert!(
        forgets_unique_named("test.ts", src_ns),
        "namespace inner(); unique-forget: {:?}",
        check_source("test.ts", src_ns).formatted_lines()
    );

    let src_jsx = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {
  <div><span>{flag && buf}</span></div>;
  consume(buf);
}
"#;
    let r = check_source("test.tsx", src_jsx);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::UniqueForget),
        "nested jsx flag && buf then consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn collect_and_and_assign_key_import_ns_dot() {
    let src_and = r#"
true && (function () {
  /*#own type: () => unique Buffer */
  function inner() { return Buffer.from("x"); }
  inner();
})();
"#;
    assert!(
        forgets_unique(src_and),
        "&& IIFE inner(); : {:?}",
        check_source("test.js", src_and).formatted_lines()
    );

    let src_key = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  const obj = {};
  obj[buf] = 1;
  consume(buf);
}
"#;
    assert!(
        has(src_key, RuleKind::UseAfterMove),
        "obj[buf] = 1 then consume: {:?}",
        check_source("test.js", src_key).formatted_lines()
    );

    let src_imp = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {
  import(flag && buf);
  consume(buf);
}
"#;
    let r = check_source("test.js", src_imp);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::UniqueForget),
        "import(flag && buf) then consume: {:?}",
        r.formatted_lines()
    );

    let src_ns = r#"
namespace N {
  /*#own type: () => unique Buffer */
  export function inner() { return Buffer.from("x"); }
}
N.inner();
"#;
    assert!(
        forgets_unique_named("test.ts", src_ns),
        "N.inner(); unique-forget: {:?}",
        check_source("test.ts", src_ns).formatted_lines()
    );
}

#[test]
fn iife_void_param_and_export_arrow() {
    let src_iife = r#"
(/*#own type: () => unique Buffer */ function () { return Buffer.from("x"); })();
"#;
    assert!(
        forgets_unique(src_iife),
        "annotated IIFE unique return: {:?}",
        check_source("test.js", src_iife).formatted_lines()
    );

    let src_end = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  process.stdout.end(buf);
  consume(buf);
}
"#;
    let r = check_source("test.js", src_end);
    assert!(
        !r.kinds().contains(&RuleKind::UseAfterMove),
        "stdout.end void param should not consume buf: {:?}",
        r.formatted_lines()
    );

    let src_export = r#"
/*#own type: () => unique Buffer */
export const f = () => Buffer.from("x");
"#;
    let r = check_source("test.js", src_export);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "export const unique arrow body: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn unknown_tagged_interp_and_ts_infer_index() {
    let src_tag = r#"
foo`${Buffer.from("x")}`;
"#;
    let r = check_source("test.js", src_tag);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "unknown tagged interp should consume like foo(Buffer.from): {:?}",
        r.formatted_lines()
    );

    let src_consume_tag = r#"
/*#own type: (ss: copy any, x: unique Buffer) => unique Buffer */
function take(ss, x) { return x; }
/*#own type: (x: unique Buffer) => void */
function consume(x) { void x; }
consume(take`${Buffer.from("x")}`);
"#;
    let r = check_source("test.js", src_consume_tag);
    assert!(
        !r.kinds().contains(&RuleKind::UniqueForget),
        "unique-returning tagged interp consumed as arg: {:?}",
        r.formatted_lines()
    );

    let src_infer = r#"
type T = U extends infer V extends { [Buffer.from("x")]: number } ? V : never;
"#;
    assert!(
        forgets_unique_named("test.ts", src_infer),
        "TSInferType constraint computed key: {:?}",
        check_source("test.ts", src_infer).formatted_lines()
    );

    let src_ctor = r#"
type T = new <U extends { [Buffer.from("x")]: number }>() => void;
"#;
    assert!(
        forgets_unique_named("test.ts", src_ctor),
        "TSConstructorType type param: {:?}",
        check_source("test.ts", src_ctor).formatted_lines()
    );

    let src_idx = r#"
type T = { [x: { [Buffer.from("x")]: number }]: string };
"#;
    assert!(
        forgets_unique_named("test.ts", src_idx),
        "index signature parameter type: {:?}",
        check_source("test.ts", src_idx).formatted_lines()
    );

    let src_this = r#"
type T = { (this: { [Buffer.from("x")]: number }): void };
"#;
    assert!(
        forgets_unique_named("test.ts", src_this),
        "call signature this param: {:?}",
        check_source("test.ts", src_this).formatted_lines()
    );

    let src_meth = r#"
type T = { m<U extends { [Buffer.from("x")]: number }>(): void };
"#;
    assert!(
        forgets_unique_named("test.ts", src_meth),
        "method signature type param: {:?}",
        check_source("test.ts", src_meth).formatted_lines()
    );

    let src_class_idx = r#"
class C {
  [x: { [Buffer.from("x")]: number }]: string;
}
"#;
    assert!(
        forgets_unique_named("test.ts", src_class_idx),
        "class index signature parameter type: {:?}",
        check_source("test.ts", src_class_idx).formatted_lines()
    );

    let src_typeof_imp = r#"
type T = typeof import("mod", { with: { k: Buffer.from("x") } });
"#;
    assert!(
        forgets_unique_named("test.ts", src_typeof_imp),
        "typeof import options unique value: {:?}",
        check_source("test.ts", src_typeof_imp).formatted_lines()
    );

    let src_field_call = r#"
class C {
  /*#own type: () => unique Buffer */
  make = () => Buffer.from("x");
}
new C().make();
"#;
    assert!(
        forgets_unique(src_field_call),
        "discarded unique class field method: {:?}",
        check_source("test.js", src_field_call).formatted_lines()
    );
}

#[test]
fn consume_unique_arg_wrappers_private_in_assign_object() {
    let src_as = r#"
/*#own type: (x: unique Buffer) => void */
function consume(x) { void x; }
consume(Buffer.from("x") as { [Buffer.from("y")]: number });
"#;
    assert!(
        forgets_unique_named("test.ts", src_as),
        "consume unique arg as-wrapper type key: {:?}",
        check_source("test.ts", src_as).formatted_lines()
    );

    let src_ta = r#"
/*#own type: (x: unique Buffer) => unique Buffer */
function take(x) { return x; }
/*#own type: (x: unique Buffer) => void */
function consume(x) { void x; }
consume(take<{ [Buffer.from("y")]: number }>(Buffer.from("x")));
"#;
    assert!(
        forgets_unique_named("test.ts", src_ta),
        "consume unique call type args: {:?}",
        check_source("test.ts", src_ta).formatted_lines()
    );

    let src_new = r#"
/*#own type: (x: unique Foo) => void */
function consume(x) { void x; }
/*#own type: () => unique Foo */
function Foo() {}
consume(new Foo<{ [Buffer.from("y")]: number }>());
"#;
    assert!(
        forgets_unique_named("test.ts", src_new),
        "consume unique new type args: {:?}",
        check_source("test.ts", src_new).formatted_lines()
    );

    let src_pin = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
class C {
  #x;
  /*#own type: (buf: unique Buffer, flag: copy boolean) => void */
  m(buf, flag) {
    #x in (flag && consume(buf));
    consume(buf);
  }
}
"#;
    let r = check_source("test.js", src_pin);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::DoubleMove),
        "private-in exclusive consume then second consume: {:?}",
        r.formatted_lines()
    );

    let src_assign = r#"
let o;
o = { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } };
o.make();
"#;
    assert!(
        forgets_unique(src_assign),
        "assigned object method unique return discarded: {:?}",
        check_source("test.js", src_assign).formatted_lines()
    );

    let src_nested = r#"
const api = { inner: { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } } };
api.inner.make();
"#;
    assert!(
        forgets_unique(src_nested),
        "nested object method unique return discarded: {:?}",
        check_source("test.js", src_nested).formatted_lines()
    );

    let src_forof = r#"
for (const o of [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }]) o.make();
"#;
    assert!(
        forgets_unique(src_forof),
        "for-of object method unique return discarded: {:?}",
        check_source("test.js", src_forof).formatted_lines()
    );

    let src_member = r#"
const o = {};
o.inner = { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } };
o.inner.make();
"#;
    assert!(
        forgets_unique(src_member),
        "assigned member object method unique return discarded: {:?}",
        check_source("test.js", src_member).formatted_lines()
    );
}

#[test]
fn unique_return_as_wrapper_and_destructure_object_methods() {
    let src_ret = r#"
/*#own type: () => unique Buffer */
function f() { return Buffer.from("x") as { [Buffer.from("y")]: number }; }
"#;
    assert!(
        forgets_unique_named("test.ts", src_ret),
        "unique return as-wrapper type key: {:?}",
        check_source("test.ts", src_ret).formatted_lines()
    );

    let src_arr = r#"
const [o] = [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }];
o.make();
"#;
    assert!(
        forgets_unique(src_arr),
        "array-destructure object method: {:?}",
        check_source("test.js", src_arr).formatted_lines()
    );

    let src_obj = r#"
const { o } = { o: { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } } };
o.make();
"#;
    assert!(
        forgets_unique(src_obj),
        "object-destructure object method: {:?}",
        check_source("test.js", src_obj).formatted_lines()
    );

    let src_assign_arr = r#"
let o;
[o] = [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }];
o.make();
"#;
    assert!(
        forgets_unique(src_assign_arr),
        "array-assign object method: {:?}",
        check_source("test.js", src_assign_arr).formatted_lines()
    );

    let src_for_ident = r#"
let o;
for (o of [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }]) o.make();
"#;
    assert!(
        forgets_unique(src_for_ident),
        "for-of assignment ident object method: {:?}",
        check_source("test.js", src_for_ident).formatted_lines()
    );

    let src_for_pat = r#"
for (const [o] of [[{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }]]) o.make();
"#;
    assert!(
        forgets_unique(src_for_pat),
        "for-of array pattern object method: {:?}",
        check_source("test.js", src_for_pat).formatted_lines()
    );

    let src_param = r#"
function f(o = { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }) { o.make(); }
"#;
    assert!(
        forgets_unique(src_param),
        "param default object method: {:?}",
        check_source("test.js", src_param).formatted_lines()
    );

    let src_quoted = r#"
const o = { /*#own type: () => unique Buffer */ "make"() { return Buffer.from("x"); } };
o.make();
"#;
    assert!(
        forgets_unique(src_quoted),
        "quoted object method key: {:?}",
        check_source("test.js", src_quoted).formatted_lines()
    );

    let src_spread = r#"
for (const o of [...[{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }]]) o.make();
"#;
    assert!(
        forgets_unique(src_spread),
        "for-of spread array object method: {:?}",
        check_source("test.js", src_spread).formatted_lines()
    );

    let src_seq = r#"
for (const o of (0, [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }])) o.make();
"#;
    assert!(
        forgets_unique(src_seq),
        "for-of sequence array object method: {:?}",
        check_source("test.js", src_seq).formatted_lines()
    );
}

#[test]
fn destructure_conditional_class_quoted_exclusive_rename() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_cond = format!(
        "const [o] = true ? [{{ {own_make} }}] : [{{ {own_make} }}]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_cond),
        "conditional array destructure object method: {:?}",
        check_source("test.js", &src_cond).formatted_lines()
    );

    let src_and = format!("const [o] = true && [{{ {own_make} }}]; o.make();\n");
    assert!(
        forgets_unique(&src_and),
        "logical array destructure object method: {:?}",
        check_source("test.js", &src_and).formatted_lines()
    );

    let src_seq = format!("const [o] = (0, [{{ {own_make} }}]); o.make();\n");
    assert!(
        forgets_unique(&src_seq),
        "sequence array destructure object method: {:?}",
        check_source("test.js", &src_seq).formatted_lines()
    );

    let src_obj_cond = format!(
        "const {{ o }} = true ? {{ o: {{ {own_make} }} }} : {{ o: {{ {own_make} }} }}; o.make();\n"
    );
    assert!(
        forgets_unique(&src_obj_cond),
        "conditional object destructure: {:?}",
        check_source("test.js", &src_obj_cond).formatted_lines()
    );

    let src_def = format!("const {{ o = {{ {own_make} }} }} = {{}}; o.make();\n");
    assert!(
        forgets_unique(&src_def),
        "object pattern default object method: {:?}",
        check_source("test.js", &src_def).formatted_lines()
    );

    let src_arr_def = format!("const [o = {{ {own_make} }}] = []; o.make();\n");
    assert!(
        forgets_unique(&src_arr_def),
        "array pattern default object method: {:?}",
        check_source("test.js", &src_arr_def).formatted_lines()
    );

    let src_rest = format!("const {{ ...o }} = {{ {own_make} }}; o.make();\n");
    assert!(
        forgets_unique(&src_rest),
        "object rest object method: {:?}",
        check_source("test.js", &src_rest).formatted_lines()
    );

    let src_quoted_class = r#"
class C {
  /*#own type: () => unique Buffer */
  "make"() { return Buffer.from("x"); }
}
new C().make();
"#;
    assert!(
        forgets_unique(src_quoted_class),
        "quoted class method unique return: {:?}",
        check_source("test.js", src_quoted_class).formatted_lines()
    );

    let src_computed_call = r#"
class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
}
new C()["make"]();
"#;
    assert!(
        forgets_unique(src_computed_call),
        "computed instance method unique return: {:?}",
        check_source("test.js", src_computed_call).formatted_lines()
    );

    let src_excl = r#"
/*#own type: (buf: unique Buffer) => void */
function consume(buf) { void buf; }
/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function f(buf, flag) {
  ({ a: x = flag && consume(buf) } = {});
  consume(buf);
}
"#;
    let r = check_source("test.js", src_excl);
    assert!(
        r.kinds().contains(&RuleKind::UseAfterMove)
            || r.kinds().contains(&RuleKind::BranchInconsistent)
            || r.kinds().contains(&RuleKind::DoubleMove),
        "renamed assignment default exclusive consume: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn param_catch_spread_numeric_anon_class_computed_this() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_param = format!("function f({{ o = {{ {own_make} }} }}) {{ o.make(); }}\n");
    assert!(
        forgets_unique(&src_param),
        "param nested default without outer init: {:?}",
        check_source("test.js", &src_param).formatted_lines()
    );

    let src_catch = format!("try {{}} catch ({{ o = {{ {own_make} }} }}) {{ o.make(); }}\n");
    assert!(
        forgets_unique(&src_catch),
        "catch pattern default object method: {:?}",
        check_source("test.js", &src_catch).formatted_lines()
    );

    let src_spread = format!("const [o] = [...[{{ {own_make} }}]]; o.make();\n");
    assert!(
        forgets_unique(&src_spread),
        "array destructure from spread: {:?}",
        check_source("test.js", &src_spread).formatted_lines()
    );

    let src_obj_spread = format!(
        "const {{ o }} = {{ ...{{ o: {{ {own_make} }} }} }}; o.make();\n"
    );
    assert!(
        forgets_unique(&src_obj_spread),
        "object destructure from spread: {:?}",
        check_source("test.js", &src_obj_spread).formatted_lines()
    );

    let src_rest_arr = format!("const [...[o]] = [{{ {own_make} }}]; o.make();\n");
    assert!(
        forgets_unique(&src_rest_arr),
        "array rest nested pattern: {:?}",
        check_source("test.js", &src_rest_arr).formatted_lines()
    );

    let src_num = r#"
class C {
  /*#own type: () => unique Buffer */
  0() { return Buffer.from("x"); }
}
new C()[0]();
"#;
    assert!(
        forgets_unique(src_num),
        "numeric class method unique return: {:?}",
        check_source("test.js", src_num).formatted_lines()
    );

    let src_anon = format!(
        "const C = class {{ {own_make} }}; new C().make();\n"
    );
    assert!(
        forgets_unique(&src_anon),
        "anonymous class bound to const: {:?}",
        check_source("test.js", &src_anon).formatted_lines()
    );

    let src_close = r#"
async function f() {
  fs.promises.open("x")["close"]();
}
"#;
    let r = check_source("test.js", src_close);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "computed FileHandle#close should move this: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn rest_defaults_mid_spread_numeric_key_array_object() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_rest = format!("function f([...[o = {{ {own_make} }}]]) {{ o.make(); }}\n");
    assert!(
        forgets_unique(&src_rest),
        "rest nested default object method: {:?}",
        check_source("test.js", &src_rest).formatted_lines()
    );

    let src_mid = format!("const [o] = [...[], {{ {own_make} }}]; o.make();\n");
    assert!(
        forgets_unique(&src_mid),
        "array destructure after empty spread: {:?}",
        check_source("test.js", &src_mid).formatted_lines()
    );

    let src_hex = r#"
class C {
  /*#own type: () => unique Buffer */
  0x0() { return Buffer.from("x"); }
}
new C()[0]();
"#;
    assert!(
        forgets_unique(src_hex),
        "hex class key vs decimal lookup: {:?}",
        check_source("test.js", src_hex).formatted_lines()
    );

    let src_obj_arr = format!("const {{ 0: o }} = [{{ {own_make} }}]; o.make();\n");
    assert!(
        forgets_unique(&src_obj_arr),
        "object destructure numeric key on array: {:?}",
        check_source("test.js", &src_obj_arr).formatted_lines()
    );
}

#[test]
fn elision_logical_spread_template_key_destructure_fn() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_elide = format!("const [, o] = [, {{ {own_make} }}]; o.make();\n");
    assert!(
        forgets_unique(&src_elide),
        "array elision keeps later slot: {:?}",
        check_source("test.js", &src_elide).formatted_lines()
    );

    let src_false = format!(
        "const [o] = [...(false ? [1] : [{{ {own_make} }}])]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_false),
        "spread of conditional array: {:?}",
        check_source("test.js", &src_false).formatted_lines()
    );

    let src_tpl = r#"
class C {
  /*#own type: () => unique Buffer */
  [`\x30`]() { return Buffer.from("x"); }
}
new C()[0]();
"#;
    assert!(
        forgets_unique(src_tpl),
        "cooked template class key: {:?}",
        check_source("test.js", src_tpl).formatted_lines()
    );

    let src_fn = r#"
const { f } = { /*#own type: () => unique Buffer */ f() { return Buffer.from("x"); } };
f();
"#;
    assert!(
        forgets_unique(src_fn),
        "destructured method bound as f(): {:?}",
        check_source("test.js", src_fn).formatted_lines()
    );
}

#[test]
fn object_pattern_spread_conditional_assign_prop_span_global_agent() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_obj = format!(
        "const {{ 0: o }} = [...(false ? [1] : [{{ {own_make} }}])]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_obj),
        "object pattern numeric key on spread conditional: {:?}",
        check_source("test.js", &src_obj).formatted_lines()
    );

    let src_assign = r#"
let f;
({ f } = { /*#own type: () => unique Buffer */ f() { return Buffer.from("x"); } });
f();
"#;
    assert!(
        forgets_unique(src_assign),
        "assignment destructure property-span type: {:?}",
        check_source("test.js", src_assign).formatted_lines()
    );

    let src_agent = r#"
http.globalAgent.on("x", () => {});
"#;
    let r = check_source("test.js", src_agent);
    assert!(
        !r.diagnostics.iter().any(|d| {
            d.kind == RuleKind::UniqueForget
                && d.message.contains("discarded without being bound")
        }),
        "http.globalAgent.on should be copy like Agent#on: {:?}",
        r.formatted_lines()
    );
}

#[test]
fn trailing_after_conditional_spread_forin_defaults_compose_unique() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_trail = format!(
        "const {{ 1: o }} = [...(false ? [1] : [1]), {{ {own_make} }}]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_trail),
        "trailing slot after equal-length conditional spread: {:?}",
        check_source("test.js", &src_trail).formatted_lines()
    );

    let src_forin = r#"
for (const { f = /*#own type: () => unique Buffer */ function() { return Buffer.from("x"); } } in { g: 1 }) f();
"#;
    assert!(
        forgets_unique(src_forin),
        "for-in left default function: {:?}",
        check_source("test.js", src_forin).formatted_lines()
    );

    let src_compose = r#"
stream.compose(process.stdin);
"#;
    assert!(
        forgets_unique(src_compose),
        "stream.compose should stay unique Duplex: {:?}",
        check_source("test.js", src_compose).formatted_lines()
    );
}

#[test]
fn and_spread_rest_enum_assign_lhs_collect_sigs() {
    let own_make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_and = format!(
        "const {{ 1: o }} = [...(true && [1]), {{ {own_make} }}]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_and),
        "trailing slot after true && [1] spread: {:?}",
        check_source("test.js", &src_and).formatted_lines()
    );

    let src_rest = format!(
        "const [...{{ 1: o }}] = [...(false ? [1] : [1]), {{ {own_make} }}]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_rest),
        "rest object pattern after conditional spread: {:?}",
        check_source("test.js", &src_rest).formatted_lines()
    );

    let src_enum = r#"
enum E {
  A = (function() {
    /*#own type: () => unique Buffer */
    function inner() { return Buffer.from("x"); }
    inner();
    return 1;
  })()
}
"#;
    assert!(
        forgets_unique_named("test.ts", src_enum),
        "enum init nested unique function call: {:?}",
        check_source("test.ts", src_enum).formatted_lines()
    );
}

#[test]
fn undefined_spread_export_enum_interface_with_decorator_collect() {
    let own_inner = r#"
/*#own type: () => unique Buffer */
function inner() { return Buffer.from("x"); }
inner();
"#;

    let src_undef = format!(
        "const {{ 1: o }} = [...(undefined ?? [1]), {{ /*#own type: () => unique Buffer */ make() {{ return Buffer.from(\"x\"); }} }}]; o.make();\n"
    );
    assert!(
        forgets_unique(&src_undef),
        "undefined ?? [1] should advance flatten origin: {:?}",
        check_source("test.js", &src_undef).formatted_lines()
    );

    let src_export_enum = format!(
        "export enum E {{ A = (function() {{ {own_inner} return 1; }})() }}\n"
    );
    assert!(
        forgets_unique_named("test.ts", &src_export_enum),
        "export enum init nested unique call: {:?}",
        check_source("test.ts", &src_export_enum).formatted_lines()
    );

    let src_iface = format!(
        "interface I {{ [(function outer() {{ {own_inner} }})()]: number }}\n"
    );
    assert!(
        forgets_unique_named("test.ts", &src_iface),
        "interface computed key nested unique call: {:?}",
        check_source("test.ts", &src_iface).formatted_lines()
    );

    let src_alias = format!(
        "type T = {{ [(function outer() {{ {own_inner} }})()]: number }};\n"
    );
    assert!(
        forgets_unique_named("test.ts", &src_alias),
        "type alias computed key nested unique call: {:?}",
        check_source("test.ts", &src_alias).formatted_lines()
    );

    let src_ann = format!(
        "const x: {{ [(function outer() {{ {own_inner} }})()]: number }} = 1;\n"
    );
    assert!(
        forgets_unique_named("test.ts", &src_ann),
        "var type annotation computed key nested unique call: {:?}",
        check_source("test.ts", &src_ann).formatted_lines()
    );

    let src_for = format!(
        "for (const x: {{ [(function outer() {{ {own_inner} }})()]: number }} of []) {{}}\n"
    );
    assert!(
        forgets_unique_named("test.ts", &src_for),
        "for-of left var annotation nested unique call: {:?}",
        check_source("test.ts", &src_for).formatted_lines()
    );

    let src_def = format!(
        "const {{ a = (function outer() {{ {own_inner} return 1; }})() }} = {{}};\n"
    );
    assert!(
        forgets_unique_named("test.js", &src_def),
        "pattern default IIFE nested unique call: {:?}",
        check_source("test.js", &src_def).formatted_lines()
    );
}

#[test]
fn new_class_expr_and_wrapped_new_instance_sig() {
    let src_named = r#"
new (class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
})().make();
"#;
    assert!(
        forgets_unique(src_named),
        "new (class C)().make discarded unique: {:?}",
        check_source("test.js", src_named).formatted_lines()
    );

    let src_seq = r#"
class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
}
(0, new C()).make();
"#;
    assert!(
        forgets_unique(src_seq),
        "sequence-wrapped new C().make discarded unique: {:?}",
        check_source("test.js", src_seq).formatted_lines()
    );

    let src_tern = r#"
class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
}
(true ? new C() : new C()).make();
"#;
    assert!(
        forgets_unique(src_tern),
        "conditional-wrapped new C().make discarded unique: {:?}",
        check_source("test.js", src_tern).formatted_lines()
    );

    let src_alias = r#"
const X = class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
};
new X().make();
"#;
    assert!(
        forgets_unique(src_alias),
        "const X = class C; new X().make discarded unique: {:?}",
        check_source("test.js", src_alias).formatted_lines()
    );

    let src_assign = r#"
new (C = class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
})().make();
"#;
    assert!(
        forgets_unique(src_assign),
        "new (C = class C)().make discarded unique: {:?}",
        check_source("test.js", src_assign).formatted_lines()
    );

    let src_bind_assign = r#"
const X = (C = class C {
  /*#own type: () => unique Buffer */
  make() { return Buffer.from("x"); }
});
new X().make();
"#;
    assert!(
        forgets_unique(src_bind_assign),
        "const X = (C = class C); new X().make: {:?}",
        check_source("test.js", src_bind_assign).formatted_lines()
    );

    let src_log = r#"
(log = console.log)(Buffer.from("x"));
"#;
    assert!(
        forgets_unique(src_log),
        "assigned console.log should unique-forget like console.log: {:?}",
        check_source("test.js", src_log).formatted_lines()
    );

    let src_pat_asgn = r#"
const [o] = (x = [{ /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } }]);
o.make();
"#;
    assert!(
        forgets_unique(src_pat_asgn),
        "array pattern init assignment wrap: {:?}",
        check_source("test.js", src_pat_asgn).formatted_lines()
    );

    let src_forof_obj = r#"
for (const { o } of (x = [{ o: { /*#own type: () => unique Buffer */ make() { return Buffer.from("x"); } } }])) o.make();
"#;
    assert!(
        forgets_unique(src_forof_obj),
        "for-of object pattern assignment wrap: {:?}",
        check_source("test.js", src_forof_obj).formatted_lines()
    );
}

#[test]
fn origin_preserving_assignment_and_call_return_assignment() {
    let make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";

    let src_key1 = format!(
        "const {{ 1: o }} = [0, ...(true && (x = [{{ {make} }}]))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_key1),
        "object pattern key 1 through logical assignment: {:?}",
        check_source("test.js", &src_key1).formatted_lines()
    );

    let src_or = format!(
        "const {{ 1: o }} = [0, ...(false || (x = [{{ {make} }}]))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_or),
        "object pattern key 1 through or assignment: {:?}",
        check_source("test.js", &src_or).formatted_lines()
    );

    let src_coal = format!(
        "const {{ 1: o }} = [0, ...(null ?? (x = [{{ {make} }}]))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_coal),
        "object pattern key 1 through coalesce assignment: {:?}",
        check_source("test.js", &src_coal).formatted_lines()
    );

    let src_tern = format!(
        "const {{ 1: o }} = [0, ...(true ? (x = [{{ {make} }}]) : (y = [{{ {make} }}]))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_tern),
        "object pattern key 1 through ternary assignment: {:?}",
        check_source("test.js", &src_tern).formatted_lines()
    );

    let src_seq_asgn = format!(
        "const {{ 1: o }} = [0, ...(true && (0, (x = [{{ {make} }}])))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_seq_asgn),
        "object pattern key 1 through sequence then assignment: {:?}",
        check_source("test.js", &src_seq_asgn).formatted_lines()
    );

    let src_for = format!(
        "for (const {{ 1: o }} of [[0, ...(true && (x = [{{ {make} }}]))]]) o.make();\n"
    );
    assert!(
        forgets_unique(&src_for),
        "for-of object pattern key 1 through logical assignment: {:?}",
        check_source("test.js", &src_for).formatted_lines()
    );

    let src_asgn_left = format!(
        "let o;\n({{ 1: o }} = [0, ...(true && (x = [{{ {make} }}]))]);\no.make();\n"
    );
    assert!(
        forgets_unique(&src_asgn_left),
        "assignment-left object pattern key 1 through logical assignment: {:?}",
        check_source("test.js", &src_asgn_left).formatted_lines()
    );

    let src_asgn_seq = format!(
        "let o;\n({{ 1: o }} = [0, ...(true && (0, [{{ {make} }}]))]);\no.make();\n"
    );
    assert!(
        forgets_unique(&src_asgn_seq),
        "assignment-left object pattern key 1 through sequence: {:?}",
        check_source("test.js", &src_asgn_seq).formatted_lines()
    );

    let src_key0 = format!(
        "const {{ 0: o }} = [...(true && (x = [{{ {make} }}]))];\no.make();\n"
    );
    assert!(
        forgets_unique(&src_key0),
        "object pattern key 0 through logical assignment still reports: {:?}",
        check_source("test.js", &src_key0).formatted_lines()
    );

    let src_bind = format!(
        "{PRELUDE}\nconst buf = (x = Buffer.from(\"x\"));\nconsume(buf);\n"
    );
    assert!(
        !has(&src_bind, RuleKind::UniqueForget),
        "assigned Buffer.from bind then consume should not unique-forget: {:?}",
        check_source("test.js", &src_bind).formatted_lines()
    );

    let src_seq_bind = format!(
        "{PRELUDE}\nconst buf = (0, Buffer.from(\"x\"));\nconsume(buf);\n"
    );
    assert!(
        !has(&src_seq_bind, RuleKind::UniqueForget),
        "sequence Buffer.from bind then consume should not unique-forget: {:?}",
        check_source("test.js", &src_seq_bind).formatted_lines()
    );

    let src_arg = format!("{PRELUDE}\nconsume(x = Buffer.from(\"x\"));\n");
    assert!(
        !has(&src_arg, RuleKind::UniqueForget),
        "consume assigned Buffer.from should not unique-forget: {:?}",
        check_source("test.js", &src_arg).formatted_lines()
    );

    let src_arg_seq = format!("{PRELUDE}\nconsume((0, Buffer.from(\"x\")));\n");
    assert!(
        !has(&src_arg_seq, RuleKind::UniqueForget),
        "consume sequence Buffer.from should not unique-forget: {:?}",
        check_source("test.js", &src_arg_seq).formatted_lines()
    );

    let src_ret = r#"
/*#own type: () => unique Buffer */
function f() { return (x = Buffer.from("x")); }
"#;
    assert!(
        !forgets_unique(src_ret),
        "return assigned Buffer.from matching unique ret should not unique-forget: {:?}",
        check_source("test.js", src_ret).formatted_lines()
    );

    let src_ret_seq = r#"
/*#own type: () => unique Buffer */
function f() { return (0, Buffer.from("x")); }
"#;
    assert!(
        !forgets_unique(src_ret_seq),
        "return sequence Buffer.from matching unique ret should not unique-forget: {:?}",
        check_source("test.js", src_ret_seq).formatted_lines()
    );
}

#[test]
fn ident_assignment_fn_init_yield_and_assign_lhs_unique() {
    let src_move = r#"
const a = Buffer.from("x");
const buf = (x = a);
"#;
    assert!(
        forgets_unique(src_move),
        "const buf = (x = a) should transfer unique like const buf = a: {:?}",
        check_source("test.js", src_move).formatted_lines()
    );

    let src_move_seq = r#"
const a = Buffer.from("x");
const buf = (0, x = a);
"#;
    assert!(
        forgets_unique(src_move_seq),
        "const buf = (0, x = a) should transfer unique: {:?}",
        check_source("test.js", src_move_seq).formatted_lines()
    );

    let src_filter = r#"
const buf = Buffer.from("x");
(x = buf).filter(() => true);
"#;
    assert!(
        forgets_unique(src_filter),
        "assigned ident receiver Buffer#filter should unique-forget like sequence: {:?}",
        check_source("test.js", src_filter).formatted_lines()
    );

    let src_filter_seq = r#"
const buf = Buffer.from("x");
(0, buf).filter(() => true);
"#;
    assert!(
        forgets_unique(src_filter_seq),
        "sequence ident receiver Buffer#filter unique-forget control: {:?}",
        check_source("test.js", src_filter_seq).formatted_lines()
    );

    let src_lhs = format!(
        "{PRELUDE}\nconsume(obj[Buffer.from(\"x\")] = Buffer.from(\"y\"));\n"
    );
    assert!(
        has(&src_lhs, RuleKind::UniqueForget),
        "unique computed key on consume assignment should unique-forget: {:?}",
        check_source("test.js", &src_lhs).formatted_lines()
    );

    let src_lhs_bind = format!(
        "{PRELUDE}\nconst buf = (obj[Buffer.from(\"x\")] = Buffer.from(\"y\"));\nconsume(buf);\n"
    );
    assert!(
        has(&src_lhs_bind, RuleKind::UniqueForget),
        "unique computed key on bound assignment should unique-forget: {:?}",
        check_source("test.js", &src_lhs_bind).formatted_lines()
    );

    let src_lhs_ret = r#"
/*#own type: () => unique Buffer */
function f() { return (obj[Buffer.from("x")] = Buffer.from("y")); }
"#;
    assert!(
        forgets_unique(src_lhs_ret),
        "unique computed key on unique return assignment should unique-forget: {:?}",
        check_source("test.js", src_lhs_ret).formatted_lines()
    );

    let src_fn_seq = r#"
/*#own type: () => unique Buffer */
const f = (0, () => Buffer.from("x"));
"#;
    assert!(
        !forgets_unique(src_fn_seq),
        "annotated arrow init through sequence should not unique-forget body: {:?}",
        check_source("test.js", src_fn_seq).formatted_lines()
    );

    let src_fn_asgn = r#"
/*#own type: () => unique Buffer */
const f = (x = () => Buffer.from("x"));
"#;
    assert!(
        !forgets_unique(src_fn_asgn),
        "annotated arrow init through assignment should not unique-forget body: {:?}",
        check_source("test.js", src_fn_asgn).formatted_lines()
    );

    let src_fn_and = r#"
/*#own type: () => unique Buffer */
const f = true && (() => Buffer.from("x"));
"#;
    assert!(
        !forgets_unique(src_fn_and),
        "annotated arrow init through and should not unique-forget body: {:?}",
        check_source("test.js", src_fn_and).formatted_lines()
    );

    let src_fn_tern = r#"
/*#own type: () => unique Buffer */
const f = true ? () => Buffer.from("x") : () => Buffer.from("y");
"#;
    assert!(
        !forgets_unique(src_fn_tern),
        "annotated arrow init through ternary should not unique-forget body: {:?}",
        check_source("test.js", src_fn_tern).formatted_lines()
    );

    let src_meth = r#"
({ /*#own type: () => unique Buffer */ m: (0, () => Buffer.from("x")) });
"#;
    assert!(
        !forgets_unique(src_meth),
        "annotated object method through sequence should not unique-forget body: {:?}",
        check_source("test.js", src_meth).formatted_lines()
    );

    let make = "/*#own type: () => unique Buffer */ make() { return Buffer.from(\"x\"); }";
    let src_yield = format!(
        "function* g() {{ const {{ 1: o }} = [0, ...(yield (x = [{{ {make} }}]))]; o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield),
        "object pattern key 1 through yield assignment: {:?}",
        check_source("test.js", &src_yield).formatted_lines()
    );

    let src_yield0 = format!(
        "function* g() {{ const {{ 0: o }} = [...(yield [{{ {make} }}])]; o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield0),
        "object pattern key 0 through yield array: {:?}",
        check_source("test.js", &src_yield0).formatted_lines()
    );

    let src_and_key = format!(
        "{PRELUDE}\nconst buf = true && (obj[Buffer.from(\"x\")] = Buffer.from(\"y\"));\nconsume(buf);\n"
    );
    assert!(
        has(&src_and_key, RuleKind::UniqueForget),
        "unique computed key through and-bound assignment should unique-forget: {:?}",
        check_source("test.js", &src_and_key).formatted_lines()
    );

    let src_tern_key = format!(
        "{PRELUDE}\nconst buf = true ? (obj[Buffer.from(\"x\")] = Buffer.from(\"y\")) : Buffer.from(\"z\");\nconsume(buf);\n"
    );
    assert!(
        has(&src_tern_key, RuleKind::UniqueForget),
        "unique computed key through ternary-bound assignment should unique-forget: {:?}",
        check_source("test.js", &src_tern_key).formatted_lines()
    );

    let src_and_filter = r#"
const buf = Buffer.from("x");
(true && buf).filter(() => true);
"#;
    assert!(
        forgets_unique(src_and_filter),
        "logical and ident receiver Buffer#filter should unique-forget: {:?}",
        check_source("test.js", src_and_filter).formatted_lines()
    );

    let src_or_filter = r#"
const buf = Buffer.from("x");
(buf || 0).filter(() => true);
"#;
    assert!(
        forgets_unique(src_or_filter),
        "logical or ident receiver Buffer#filter should unique-forget: {:?}",
        check_source("test.js", src_or_filter).formatted_lines()
    );

    let src_tern_filter = r#"
const buf = Buffer.from("x");
(true ? buf : buf).filter(() => true);
"#;
    assert!(
        forgets_unique(src_tern_filter),
        "ternary ident receiver Buffer#filter should unique-forget: {:?}",
        check_source("test.js", src_tern_filter).formatted_lines()
    );

    let src_yield_pat = format!(
        "function* g() {{ const {{ 0: o }} = yield [{{ {make} }}]; o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield_pat),
        "origin-0 object pattern through yield array: {:?}",
        check_source("test.js", &src_yield_pat).formatted_lines()
    );

    let src_yield_arr = format!(
        "function* g() {{ const [o] = yield [{{ {make} }}]; o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield_arr),
        "array pattern through yield array: {:?}",
        check_source("test.js", &src_yield_arr).formatted_lines()
    );

    let src_yield_for = format!(
        "function* g() {{ for (const {{ o }} of yield [{{ o: {{ {make} }} }}]) o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield_for),
        "for-of object pattern through yield array: {:?}",
        check_source("test.js", &src_yield_for).formatted_lines()
    );

    let src_yield_obj = format!(
        "function* g() {{ const o = yield {{ {make} }}; o.make(); }}\n"
    );
    assert!(
        forgets_unique(&src_yield_obj),
        "object init through yield: {:?}",
        check_source("test.js", &src_yield_obj).formatted_lines()
    );

    let src_ident_asgn_key = r#"
const a = Buffer.from("x");
const buf = (obj[Buffer.from("y")] = a);
"#;
    assert!(
        forgets_unique(src_ident_asgn_key),
        "ident-move assignment target unique should unique-forget: {:?}",
        check_source("test.js", src_ident_asgn_key).formatted_lines()
    );

    let src_or_from = r#"
const a = Buffer.from("x");
const buf = a || Buffer.from("y");
"#;
    assert!(
        forgets_unique(src_or_from),
        "a || Buffer.from should unique-forget the call: {:?}",
        check_source("test.js", src_or_from).formatted_lines()
    );

    let src_from_or = format!(
        "{PRELUDE}\nconst a = Buffer.from(\"x\");\nconst buf = Buffer.from(\"y\") || a;\nconsume(buf);\n"
    );
    let from_or = check_source("test.js", &src_from_or);
    assert!(
        from_or.kinds().contains(&RuleKind::UniqueForget)
            || from_or.kinds().contains(&RuleKind::BranchInconsistent),
        "Buffer.from || a should not drop the unique call: {:?}",
        from_or.formatted_lines()
    );
}
