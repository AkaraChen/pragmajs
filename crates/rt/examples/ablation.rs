use pragma_rt::checker::check_source_with_environment_and_features;
use pragma_rt::parser;
use pragma_rt::prelude::Environment;
use pragma_rt::syntax::{Annotation, RtError};
use pragma_rt::verifier::{RtAblation, RtFeatures};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Case {
    path: PathBuf,
    source: String,
    annotations: Vec<Annotation>,
    environment: Environment,
    should_accept: bool,
}

#[derive(Default)]
struct Score {
    valid_kept: usize,
    lost_valid: usize,
    invalid_caught: usize,
    escaped_invalid: usize,
}

struct Run {
    name: &'static str,
    score: Score,
    diagnostics: Vec<Vec<RtError>>,
    median: Duration,
}

fn collect_files(dir: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_files(&path, output)?;
        } else {
            output.push(path);
        }
    }
    Ok(())
}

fn environment_for(path: &Path) -> Environment {
    let rendered = path.to_string_lossy();
    if rendered.contains("/node/") {
        Environment::Node
    } else if rendered.contains("/deno/") {
        Environment::Deno
    } else if rendered.contains("/bun/") {
        Environment::Bun
    } else if rendered.contains("/dom/") {
        Environment::Browser
    } else {
        Environment::Ecmascript
    }
}

fn load_cases() -> Result<Vec<Case>, String> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let mut paths = Vec::new();
    collect_files(&fixtures, &mut paths)?;
    paths.sort();
    let mut cases = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_flux = name.starts_with("flux_");
        let is_prelude = path.to_string_lossy().contains("/fixtures/prelude/");
        let should_accept = name.ends_with("_positive.js");
        let should_reject = name.ends_with("_negative.js");
        if (!is_flux && !is_prelude) || (!should_accept && !should_reject) {
            continue;
        }
        let source =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let file_name = path.to_string_lossy();
        let parsed = parser::parse_file(&source, &file_name)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        cases.push(Case {
            environment: if is_flux {
                Environment::Auto
            } else {
                environment_for(&path)
            },
            path,
            source,
            annotations: parsed.annotations,
            should_accept,
        });
    }
    Ok(cases)
}

fn check(case: &Case, features: RtFeatures) -> Vec<RtError> {
    check_source_with_environment_and_features(
        &case.source,
        &case.path.to_string_lossy(),
        &case.annotations,
        case.environment,
        features,
    )
}

fn run_once(cases: &[Case], features: RtFeatures) -> Vec<Vec<RtError>> {
    cases.iter().map(|case| check(case, features)).collect()
}

fn evaluate(name: &'static str, cases: &[Case], features: RtFeatures, rounds: usize) -> Run {
    let diagnostics = run_once(cases, features);
    let mut score = Score::default();
    for (case, errors) in cases.iter().zip(&diagnostics) {
        match (case.should_accept, errors.is_empty()) {
            (true, true) => score.valid_kept += 1,
            (true, false) => score.lost_valid += 1,
            (false, false) => score.invalid_caught += 1,
            (false, true) => score.escaped_invalid += 1,
        }
    }
    let mut durations = Vec::new();
    for _ in 0..rounds {
        let started = Instant::now();
        let _ = run_once(cases, features);
        durations.push(started.elapsed());
    }
    durations.sort_unstable();
    Run {
        name,
        score,
        diagnostics,
        median: durations[durations.len() / 2],
    }
}

fn same_diagnostics(left: &[RtError], right: &[RtError]) -> bool {
    left == right
}

fn report_delta(baseline: &Run, run: &Run, cases: &[Case]) {
    let changed_acceptance = baseline
        .diagnostics
        .iter()
        .zip(&run.diagnostics)
        .filter(|(left, right)| left.is_empty() != right.is_empty())
        .count();
    let changed_diagnostics = baseline
        .diagnostics
        .iter()
        .zip(&run.diagnostics)
        .filter(|(left, right)| !same_diagnostics(left, right))
        .count();
    println!(
        "{} vs baseline: changed acceptance {changed_acceptance}; changed exact diagnostic lists {changed_diagnostics}",
        run.name
    );
    if changed_diagnostics != 0 {
        println!("{} changed files:", run.name);
        for ((case, left), right) in cases
            .iter()
            .zip(&baseline.diagnostics)
            .zip(&run.diagnostics)
        {
            if !same_diagnostics(left, right) {
                println!("- {}", case.path.display());
            }
        }
    }
}

fn main() -> Result<(), String> {
    let cases = load_cases()?;
    let rounds = env::var("ABLATION_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let baseline_features = RtFeatures::default();
    let baseline = evaluate("direct-smt (production)", &cases, baseline_features, rounds);
    let no_int_conversion_axioms = evaluate(
        "no-int-conversion-axioms",
        &cases,
        baseline_features.without(RtAblation::IntConversionAxioms),
        rounds,
    );
    let no_abstract_predicate_congruence = evaluate(
        "no-abstract-predicate-congruence",
        &cases,
        baseline_features.without(RtAblation::AbstractPredicateCongruence),
        rounds,
    );
    let no_heap_fact_invalidation = evaluate(
        "no-heap-fact-invalidation",
        &cases,
        baseline_features.without(RtAblation::HeapFactInvalidation),
        rounds,
    );

    println!(
        "cases: {} (Flux + prelude; parsed without Corsa)",
        cases.len()
    );
    println!("timing: median of {rounds} full-corpus runs (includes identical parse overhead)");
    println!(
        "| configuration | valid kept | lost valid | invalid caught | escaped invalid | median ms |"
    );
    println!("|---|---:|---:|---:|---:|---:|");
    for run in [
        &baseline,
        &no_int_conversion_axioms,
        &no_abstract_predicate_congruence,
        &no_heap_fact_invalidation,
    ] {
        println!(
            "| {} | {} | {} | {} | {} | {:.3} |",
            run.name,
            run.score.valid_kept,
            run.score.lost_valid,
            run.score.invalid_caught,
            run.score.escaped_invalid,
            run.median.as_secs_f64() * 1000.0,
        );
    }
    for run in [
        &no_int_conversion_axioms,
        &no_abstract_predicate_congruence,
        &no_heap_fact_invalidation,
    ] {
        report_delta(&baseline, run, &cases);
    }
    Ok(())
}
