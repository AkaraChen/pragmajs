//! Tests drive the shipped `check_source` / `check_paths` APIs — the same
//! functions the CLI calls. No mock checker.

use std::collections::HashMap;
use std::path::PathBuf;

use ownershipjs::{check_paths, check_source, check_source_with, RuleKind, Runtime};

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

/// Same entry point the CLI uses (`check_paths`) on the real `examples/` tree.
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
