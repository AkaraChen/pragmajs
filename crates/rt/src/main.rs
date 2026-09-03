use pragma_rt::{checker, parser, prelude, runtime, transpiler};

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

const TARGET_VALUES: &str = "auto, ecmascript, browser, node, deno, bun";

#[derive(Debug, PartialEq, Eq)]
struct CompilerOptions {
    corsa_path: String,
    tsconfig_path: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Check {
        input_path: String,
        environment: prelude::Environment,
        compiler: Option<CompilerOptions>,
    },
    Build {
        input_path: String,
        output_path: String,
        environment: prelude::Environment,
        compiler: Option<CompilerOptions>,
    },
}

fn parse_target(value: &str) -> Result<prelude::Environment, String> {
    match value {
        "auto" => Ok(prelude::Environment::Auto),
        "ecmascript" => Ok(prelude::Environment::Ecmascript),
        "browser" => Ok(prelude::Environment::Browser),
        "node" => Ok(prelude::Environment::Node),
        "deno" => Ok(prelude::Environment::Deno),
        "bun" => Ok(prelude::Environment::Bun),
        _ => Err(format!(
            "Unknown target '{value}'. Expected one of: {TARGET_VALUES}."
        )),
    }
}

fn parse_options(
    args: &[String],
) -> Result<(prelude::Environment, Option<CompilerOptions>, Vec<String>), String> {
    let mut environment = prelude::Environment::Auto;
    let mut target_seen = false;
    let mut corsa_path = None;
    let mut tsconfig_path = None;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let argument = &args[index];
        let target = if argument == "--target" {
            index += 1;
            Some(
                args.get(index)
                    .ok_or_else(|| {
                        format!("Missing value for '--target'. Expected one of: {TARGET_VALUES}.")
                    })?
                    .as_str(),
            )
        } else {
            argument.strip_prefix("--target=")
        };

        if let Some(target) = target {
            if target_seen {
                return Err("Target specified more than once.".to_string());
            }
            environment = parse_target(target)?;
            target_seen = true;
        } else if argument == "--corsa" || argument.starts_with("--corsa=") {
            if corsa_path.is_some() {
                return Err("Corsa executable specified more than once.".to_string());
            }
            let value = if argument == "--corsa" {
                index += 1;
                args.get(index)
                    .ok_or_else(|| "Missing value for '--corsa'.".to_string())?
                    .as_str()
            } else {
                argument.trim_start_matches("--corsa=")
            };
            if value.is_empty() {
                return Err("Missing value for '--corsa'.".to_string());
            }
            corsa_path = Some(value.to_string());
        } else if argument == "--tsconfig" || argument.starts_with("--tsconfig=") {
            if tsconfig_path.is_some() {
                return Err("TypeScript config specified more than once.".to_string());
            }
            let value = if argument == "--tsconfig" {
                index += 1;
                args.get(index)
                    .ok_or_else(|| "Missing value for '--tsconfig'.".to_string())?
                    .as_str()
            } else {
                argument.trim_start_matches("--tsconfig=")
            };
            if value.is_empty() {
                return Err("Missing value for '--tsconfig'.".to_string());
            }
            tsconfig_path = Some(value.to_string());
        } else if argument.starts_with('-') {
            return Err(format!("Unknown option '{argument}'."));
        } else {
            positional.push(argument.clone());
        }

        index += 1;
    }

    let compiler = match (corsa_path, tsconfig_path) {
        (Some(corsa_path), Some(tsconfig_path)) => Some(CompilerOptions {
            corsa_path,
            tsconfig_path,
        }),
        (Some(_), None) => {
            return Err("'--corsa' requires '--tsconfig'.".to_string());
        }
        (None, Some(_)) => {
            return Err("'--tsconfig' requires '--corsa'.".to_string());
        }
        (None, None) => None,
    };

    Ok((environment, compiler, positional))
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let Some((command, command_args)) = args.split_first() else {
        return Err("Usage: pragma-rt <check|build> [args]".to_string());
    };

    match command.as_str() {
        "check" => {
            let (environment, compiler, positional) = parse_options(command_args)?;
            if positional.len() != 1 {
                return Err(format!(
                    "Usage: pragma-rt check [--target <{TARGET_VALUES}>] [--corsa <executable> --tsconfig <file>] <file>"
                ));
            }
            Ok(Command::Check {
                input_path: positional[0].clone(),
                environment,
                compiler,
            })
        }
        "build" => {
            let (environment, compiler, positional) = parse_options(command_args)?;
            if positional.len() != 2 {
                return Err(format!(
                    "Usage: pragma-rt build [--target <{TARGET_VALUES}>] [--corsa <executable> --tsconfig <file>] <input> <output>"
                ));
            }
            Ok(Command::Build {
                input_path: positional[0].clone(),
                output_path: positional[1].clone(),
                environment,
                compiler,
            })
        }
        _ => Err("Unknown command. Use 'check' or 'build'.".to_string()),
    }
}

fn canonicalize_cli_path(path: &str, role: &str) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("Failed to resolve {role} '{path}': {error}"))
}

fn check_with_options(
    source: &str,
    input_path: &str,
    annotations: &[pragma_rt::syntax::Annotation],
    environment: prelude::Environment,
    compiler: Option<&CompilerOptions>,
) -> Result<Vec<pragma_rt::syntax::RtError>, String> {
    let Some(compiler) = compiler else {
        return Ok(checker::check_source_with_environment(
            source,
            input_path,
            annotations,
            environment,
        ));
    };

    let source_path = canonicalize_cli_path(input_path, "source file")?;
    let corsa_path = canonicalize_cli_path(&compiler.corsa_path, "Corsa executable")?;
    let config_path = canonicalize_cli_path(&compiler.tsconfig_path, "TypeScript config")?;
    let working_directory = config_path
        .parent()
        .ok_or_else(|| {
            format!(
                "TypeScript config '{}' has no parent directory",
                config_path.display()
            )
        })?
        .to_path_buf();
    let provider = pragma_rt::type_provider::CorsaTypeProvider::new(corsa_path, working_directory);
    checker::check_source_with_environment_and_compiler(
        source,
        input_path,
        annotations,
        environment,
        &provider,
        &config_path,
        &source_path,
    )
    .map_err(|error| format!("Compiler type analysis failed: {error}"))
}

fn run_check(
    input_path: &str,
    environment: prelude::Environment,
    compiler: Option<&CompilerOptions>,
) -> bool {
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input_path, e);
            return false;
        }
    };

    let result = match parser::parse_file(&source, input_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return false;
        }
    };

    let errors = match check_with_options(
        &source,
        input_path,
        &result.annotations,
        environment,
        compiler,
    ) {
        Ok(errors) => errors,
        Err(error) => {
            eprintln!("{error}");
            return false;
        }
    };
    if !errors.is_empty() {
        for e in errors {
            let loc = e
                .loc
                .as_ref()
                .map(|l| {
                    format!(
                        "{}:{}:{}: ",
                        l.file.as_deref().unwrap_or(""),
                        l.line,
                        l.column
                    )
                })
                .unwrap_or_default();
            eprintln!("{}{}", loc, e.message);
        }
        return false;
    }

    println!("No refinement errors found.");
    true
}

fn run_build(
    input_path: &str,
    output_path: &str,
    environment: prelude::Environment,
    compiler: Option<&CompilerOptions>,
) -> bool {
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input_path, e);
            return false;
        }
    };

    let result = match parser::parse_file(&source, input_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return false;
        }
    };

    let errors = match check_with_options(
        &source,
        input_path,
        &result.annotations,
        environment,
        compiler,
    ) {
        Ok(errors) => errors,
        Err(error) => {
            eprintln!("{error}");
            return false;
        }
    };
    if !errors.is_empty() {
        for e in errors {
            let loc = e
                .loc
                .as_ref()
                .map(|l| {
                    format!(
                        "{}:{}:{}: ",
                        l.file.as_deref().unwrap_or(""),
                        l.line,
                        l.column
                    )
                })
                .unwrap_or_default();
            eprintln!("{}{}", loc, e.message);
        }
        return false;
    }

    let transformed = match transpiler::transpile(&source, &result.annotations) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Transpile error: {}", e);
            return false;
        }
    };

    let output = format!("{}\n\n{}", runtime::runtime_block(), transformed);
    if let Err(e) = fs::write(output_path, output) {
        eprintln!("Failed to write {}: {}", output_path, e);
        return false;
    }

    println!("Wrote {}", output_path);
    true
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse_command(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    };

    let succeeded = match command {
        Command::Check {
            input_path,
            environment,
            compiler,
        } => run_check(&input_path, environment, compiler.as_ref()),
        Command::Build {
            input_path,
            output_path,
            environment,
            compiler,
        } => run_build(&input_path, &output_path, environment, compiler.as_ref()),
    };
    if !succeeded {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn positional_check_defaults_to_auto() {
        assert_eq!(
            parse_command(&args(&["check", "input.js"])),
            Ok(Command::Check {
                input_path: "input.js".to_string(),
                environment: prelude::Environment::Auto,
                compiler: None,
            })
        );
    }

    #[test]
    fn positional_build_defaults_to_auto() {
        assert_eq!(
            parse_command(&args(&["build", "input.js", "output.js"])),
            Ok(Command::Build {
                input_path: "input.js".to_string(),
                output_path: "output.js".to_string(),
                environment: prelude::Environment::Auto,
                compiler: None,
            })
        );
    }

    #[test]
    fn target_can_precede_or_follow_positional_arguments() {
        assert_eq!(
            parse_command(&args(&["check", "--target", "node", "input.js"])),
            Ok(Command::Check {
                input_path: "input.js".to_string(),
                environment: prelude::Environment::Node,
                compiler: None,
            })
        );
        assert_eq!(
            parse_command(&args(&["build", "input.js", "output.js", "--target=bun"])),
            Ok(Command::Build {
                input_path: "input.js".to_string(),
                output_path: "output.js".to_string(),
                environment: prelude::Environment::Bun,
                compiler: None,
            })
        );
    }

    #[test]
    fn all_documented_targets_are_accepted() {
        for (target, environment) in [
            ("auto", prelude::Environment::Auto),
            ("ecmascript", prelude::Environment::Ecmascript),
            ("browser", prelude::Environment::Browser),
            ("node", prelude::Environment::Node),
            ("deno", prelude::Environment::Deno),
            ("bun", prelude::Environment::Bun),
        ] {
            assert_eq!(parse_target(target), Ok(environment));
        }
    }

    #[test]
    fn unknown_target_has_deterministic_diagnostic() {
        assert_eq!(
            parse_command(&args(&["check", "--target", "workerd", "input.js"])),
            Err(
                "Unknown target 'workerd'. Expected one of: auto, ecmascript, browser, node, deno, bun."
                    .to_string()
            )
        );
    }

    #[test]
    fn target_requires_a_value() {
        assert_eq!(
            parse_command(&args(&["check", "input.js", "--target"])),
            Err(
                "Missing value for '--target'. Expected one of: auto, ecmascript, browser, node, deno, bun."
                    .to_string()
            )
        );
    }

    #[test]
    fn compiler_options_must_be_supplied_as_a_pair() {
        assert_eq!(
            parse_command(&args(&[
                "check",
                "--corsa",
                "/tools/corsa",
                "--tsconfig=/project/tsconfig.json",
                "input.js",
            ])),
            Ok(Command::Check {
                input_path: "input.js".to_string(),
                environment: prelude::Environment::Auto,
                compiler: Some(CompilerOptions {
                    corsa_path: "/tools/corsa".to_string(),
                    tsconfig_path: "/project/tsconfig.json".to_string(),
                }),
            })
        );
        assert_eq!(
            parse_command(&args(&["check", "--corsa", "/tools/corsa", "input.js"])),
            Err("'--corsa' requires '--tsconfig'.".to_string())
        );
        assert_eq!(
            parse_command(&args(&[
                "check",
                "--tsconfig",
                "/project/tsconfig.json",
                "input.js",
            ])),
            Err("'--tsconfig' requires '--corsa'.".to_string())
        );
    }
}
