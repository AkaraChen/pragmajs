use std::env;
use std::path::PathBuf;
use std::process;

use ownershipjs::Runtime;

fn main() {
    let mut args = env::args().skip(1);
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut saw_check = false;
    let mut runtime = Runtime::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" | "-c" => saw_check = true,
            "--help" | "-h" => {
                print_help();
                return;
            }
            "--version" | "-V" => {
                println!("ownershipjs {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--runtime" | "-r" => {
                let Some(v) = args.next() else {
                    eprintln!("error: --runtime needs node, bun, deno, or none");
                    process::exit(2);
                };
                runtime = parse_runtime(&v);
            }
            other => {
                if let Some(v) = other.strip_prefix("--runtime=") {
                    runtime = parse_runtime(v);
                } else {
                    paths.push(PathBuf::from(other));
                }
            }
        }
    }
    let _ = saw_check;
    if paths.is_empty() {
        eprintln!("usage: ownershipjs --check [--runtime node|bun|deno|none] <file-or-dir>");
        process::exit(2);
    }
    match ownershipjs::check_paths_with(&paths, runtime) {
        Ok(result) => {
            for line in result.formatted_lines() {
                println!("{line}");
            }
            if result.failed() {
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(2);
        }
    }
}

fn parse_runtime(v: &str) -> Runtime {
    match Runtime::parse(v) {
        Some(r) => r,
        None => {
            eprintln!("error: unknown runtime `{v}` (expected node, bun, deno, or none)");
            process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "ownershipjs — static ownership/borrow checker for JS/TS\n\n\
         Usage:\n    ownershipjs --check [--runtime node|bun|deno|none] <file-or-dir>...\n\n\
         --runtime, -r   Builtin prelude: node (default), bun, deno, or none\n\n\
         Reads /*#own ... */ comments and reports move, borrow, and lifetime errors.\n\
         Does not generate or inject runtime code."
    );
}
