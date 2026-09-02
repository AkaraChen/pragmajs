use std::env;
use std::path::PathBuf;
use std::process;

fn main() {
    let mut args = env::args().skip(1);
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut saw_check = false;
    for a in args.by_ref() {
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
            other => paths.push(PathBuf::from(other)),
        }
    }
    let _ = saw_check;
    if paths.is_empty() {
        eprintln!("usage: ownershipjs --check <file-or-dir>");
        process::exit(2);
    }
    match ownershipjs::check_paths(&paths) {
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

fn print_help() {
    println!(
        "ownershipjs — static ownership/borrow checker for JS/TS\n\n\
         Usage:\n    ownershipjs --check <file-or-dir>...\n\n\
         Reads /*#own ... */ comments and reports move, borrow, and lifetime errors.\n\
         Does not generate or inject runtime code."
    );
}
