use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use pragma_parse::{parse, Allocator};
use pragmajs::{
    check_parsed, collect_js_ts_files, emit_source, help_text, parse_args, CheckOptions, Command,
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            process::exit(2);
        }
    };

    let succeeded = match command {
        Command::Help => {
            println!("{}", help_text());
            true
        }
        Command::Version => {
            println!("pragmajs {}", env!("CARGO_PKG_VERSION"));
            true
        }
        Command::Check { paths, options } => run_check(&paths, &options),
        Command::Build {
            input_path,
            output_path,
            options,
        } => run_build(&input_path, &output_path, &options),
    };
    if !succeeded {
        process::exit(1);
    }
}

fn run_check(paths: &[PathBuf], options: &CheckOptions) -> bool {
    let mut files = Vec::new();
    for path in paths {
        if let Err(error) = collect_js_ts_files(path, &mut files) {
            eprintln!("error: {error}");
            return false;
        }
    }
    files.sort();
    if files.is_empty() {
        eprintln!("error: no JavaScript or TypeScript files to check");
        return false;
    }

    let mut ok = true;
    for file in files {
        match check_path(&file, options) {
            Ok(result) => {
                for line in result.formatted_lines() {
                    println!("{line}");
                }
                if result.failed() {
                    ok = false;
                }
            }
            Err(error) => {
                eprintln!("error: {error}");
                return false;
            }
        }
    }
    ok
}

fn check_path(
    path: &std::path::Path,
    options: &CheckOptions,
) -> Result<pragmajs::CombinedCheck, pragmajs::CombinedError> {
    let source = fs::read_to_string(path)?;
    let filename = path.to_string_lossy().to_string();
    let allocator = Allocator::new();
    let parsed = parse(&allocator, &filename, &source);
    check_parsed(&filename, &source, &parsed, options)
}

fn run_build(input_path: &str, output_path: &str, options: &CheckOptions) -> bool {
    let source = match fs::read_to_string(input_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("Failed to read {input_path}: {error}");
            return false;
        }
    };

    let emitted = match emit_source(input_path, &source, options) {
        Ok(emitted) => emitted,
        Err(error) => {
            eprintln!("error: {error}");
            return false;
        }
    };

    if emitted.check.failed() {
        for line in emitted.check.formatted_lines() {
            eprintln!("{line}");
        }
        return false;
    }

    let Some(code) = emitted.code else {
        eprintln!("error: emit produced no output");
        return false;
    };
    if let Err(error) = fs::write(output_path, code) {
        eprintln!("Failed to write {output_path}: {error}");
        return false;
    }
    println!("Wrote {output_path}");
    true
}
