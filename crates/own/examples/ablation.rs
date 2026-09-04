use pragma_own::{check_source_with_features, OwnAblation, OwnFeatures, RuleKind, Runtime};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum Label {
    Accept,
    Reject(RuleKind),
    OutOfDomain(RuleKind),
}

struct Case {
    name: String,
    path: PathBuf,
    source: String,
    label: Label,
}

#[derive(Default)]
struct Score {
    valid_kept: usize,
    lost_valid: usize,
    invalid_caught: usize,
    escaped_invalid: usize,
    reason_changed: usize,
    domain_guarded: usize,
    domain_unguarded: usize,
}

struct Run {
    name: String,
    score: Score,
    outcomes: Vec<Vec<RuleKind>>,
}

struct InteractionRuns {
    left: &'static str,
    right: &'static str,
    /// Run indices for 11, 10, 01, and 00, where 1 means enabled.
    cells: [usize; 4],
}

struct Contrast {
    valid_kept: isize,
    lost_valid: isize,
    invalid_caught: isize,
    escaped_invalid: isize,
    reason_changed: isize,
    domain_guarded: isize,
    domain_unguarded: isize,
}

fn rule(slug: &str) -> Result<RuleKind, String> {
    let kind = match slug {
        "unique-forget" => RuleKind::UniqueForget,
        "double-move" => RuleKind::DoubleMove,
        "use-after-move" => RuleKind::UseAfterMove,
        "consume-in-loop" => RuleKind::ConsumeInLoop,
        "branch-inconsistent" => RuleKind::BranchInconsistent,
        "borrow-after-move" => RuleKind::BorrowAfterMove,
        "consume-while-borrowed" => RuleKind::ConsumeWhileBorrowed,
        "mut-borrow-conflict" => RuleKind::MutBorrowConflict,
        "borrow-escape" => RuleKind::BorrowEscape,
        "unmapped" => RuleKind::UnmappedConstruct,
        "annot-parse" => RuleKind::AnnotParseError,
        "missing-type" => RuleKind::MissingType,
        _ => return Err(format!("unknown rule slug `{slug}`")),
    };
    Ok(kind)
}

fn label(raw: &str) -> Result<Label, String> {
    if raw == "ACCEPT" {
        return Ok(Label::Accept);
    }
    if let Some(slug) = raw.strip_prefix("REJECT:") {
        return Ok(Label::Reject(rule(slug)?));
    }
    if let Some(slug) = raw.strip_prefix("OUT_OF_DOMAIN:") {
        return Ok(Label::OutOfDomain(rule(slug)?));
    }
    Err(format!("unknown label `{raw}`"))
}

fn load_cases() -> Result<Vec<Case>, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("ablation");
    let manifest_path = root.join("manifest.tsv");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let mut cases = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (relative, raw_label) = line
            .split_once('\t')
            .ok_or_else(|| format!("manifest line {} has no tab", index + 1))?;
        let path = root.join(relative);
        let source =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        cases.push(Case {
            name: relative.to_string(),
            path,
            source,
            label: label(raw_label)?,
        });
    }
    Ok(cases)
}

fn evaluate(name: &str, cases: &[Case], features: OwnFeatures, runtime: Runtime) -> Run {
    let mut score = Score::default();
    let mut outcomes = Vec::new();
    for case in cases {
        let filename = case.path.to_string_lossy();
        let result = check_source_with_features(&filename, &case.source, runtime, features);
        let kinds = result.kinds();
        match case.label {
            Label::Accept if kinds.is_empty() => score.valid_kept += 1,
            Label::Accept => score.lost_valid += 1,
            Label::Reject(expected) if kinds.contains(&expected) => score.invalid_caught += 1,
            Label::Reject(_) if kinds.is_empty() => score.escaped_invalid += 1,
            Label::Reject(_) => score.reason_changed += 1,
            Label::OutOfDomain(expected) if kinds.contains(&expected) => score.domain_guarded += 1,
            Label::OutOfDomain(_) => score.domain_unguarded += 1,
        }
        outcomes.push(kinds);
    }
    Run {
        name: name.to_string(),
        score,
        outcomes,
    }
}

fn normalized(kinds: &[RuleKind]) -> String {
    let mut slugs: Vec<_> = kinds.iter().map(|kind| kind.slug()).collect();
    slugs.sort_unstable();
    slugs.dedup();
    if slugs.is_empty() {
        "ACCEPT".to_string()
    } else {
        slugs.join("+")
    }
}

fn expected(label: Label) -> String {
    match label {
        Label::Accept => "ACCEPT".to_string(),
        Label::Reject(kind) => format!("REJECT:{}", kind.slug()),
        Label::OutOfDomain(kind) => format!("OUT_OF_DOMAIN:{}", kind.slug()),
    }
}

fn matches_gold(label: Label, kinds: &[RuleKind]) -> bool {
    match label {
        Label::Accept => kinds.is_empty(),
        Label::Reject(kind) | Label::OutOfDomain(kind) => kinds.contains(&kind),
    }
}

fn changed_cases<'a>(baseline: &Run, run: &Run, cases: &'a [Case]) -> Vec<&'a str> {
    baseline
        .outcomes
        .iter()
        .zip(&run.outcomes)
        .zip(cases)
        .filter_map(|((before, after), case)| {
            (normalized(before) != normalized(after)).then_some(case.name.as_str())
        })
        .collect()
}

fn add_interaction(
    runs: &mut Vec<Run>,
    cases: &[Case],
    left: &'static str,
    right: &'static str,
    set_features: impl Fn(&mut OwnFeatures, bool, bool),
) -> InteractionRuns {
    let mut cells = [0; 4];
    for (cell, (left_enabled, right_enabled)) in
        [(true, true), (true, false), (false, true), (false, false)]
            .into_iter()
            .enumerate()
    {
        let mut features = OwnFeatures::all();
        set_features(&mut features, left_enabled, right_enabled);
        cells[cell] = runs.len();
        runs.push(evaluate(
            &format!(
                "2x2:{left}={}*{right}={}",
                if left_enabled { "on" } else { "off" },
                if right_enabled { "on" } else { "off" },
            ),
            cases,
            features,
            Runtime::Node,
        ));
    }
    InteractionRuns { left, right, cells }
}

fn contrast_value(on_on: usize, on_off: usize, off_on: usize, off_off: usize) -> isize {
    on_on as isize - on_off as isize - off_on as isize + off_off as isize
}

fn interaction_contrast(interaction: &InteractionRuns, runs: &[Run]) -> Contrast {
    let [on_on, on_off, off_on, off_off] = interaction.cells.map(|index| &runs[index].score);
    Contrast {
        valid_kept: contrast_value(
            on_on.valid_kept,
            on_off.valid_kept,
            off_on.valid_kept,
            off_off.valid_kept,
        ),
        lost_valid: contrast_value(
            on_on.lost_valid,
            on_off.lost_valid,
            off_on.lost_valid,
            off_off.lost_valid,
        ),
        invalid_caught: contrast_value(
            on_on.invalid_caught,
            on_off.invalid_caught,
            off_on.invalid_caught,
            off_off.invalid_caught,
        ),
        escaped_invalid: contrast_value(
            on_on.escaped_invalid,
            on_off.escaped_invalid,
            off_on.escaped_invalid,
            off_off.escaped_invalid,
        ),
        reason_changed: contrast_value(
            on_on.reason_changed,
            on_off.reason_changed,
            off_on.reason_changed,
            off_off.reason_changed,
        ),
        domain_guarded: contrast_value(
            on_on.domain_guarded,
            on_off.domain_guarded,
            off_on.domain_guarded,
            off_off.domain_guarded,
        ),
        domain_unguarded: contrast_value(
            on_on.domain_unguarded,
            on_off.domain_unguarded,
            off_on.domain_unguarded,
            off_off.domain_unguarded,
        ),
    }
}

fn print_csv(runs: &[Run], interactions: &[InteractionRuns], baseline: &Run, cases: &[Case]) {
    println!("variant,valid_kept,lost_valid,invalid_caught,escaped_invalid,reason_changed,domain_guarded,domain_unguarded,changed_cases");
    for run in runs {
        let score = &run.score;
        println!(
            "{},{},{},{},{},{},{},{},{}",
            run.name,
            score.valid_kept,
            score.lost_valid,
            score.invalid_caught,
            score.escaped_invalid,
            score.reason_changed,
            score.domain_guarded,
            score.domain_unguarded,
            changed_cases(baseline, run, cases).len(),
        );
    }
    for interaction in interactions {
        let contrast = interaction_contrast(interaction, runs);
        println!(
            "contrast[11-10-01+00]:{}*{},{},{},{},{},{},{},{},",
            interaction.left,
            interaction.right,
            contrast.valid_kept,
            contrast.lost_valid,
            contrast.invalid_caught,
            contrast.escaped_invalid,
            contrast.reason_changed,
            contrast.domain_guarded,
            contrast.domain_unguarded,
        );
    }
}

fn print_markdown(runs: &[Run], interactions: &[InteractionRuns], baseline: &Run, cases: &[Case]) {
    println!("| variant | valid kept | lost valid | invalid caught | escaped invalid | reason changed | OOD guarded | changed |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for run in runs {
        let score = &run.score;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            run.name,
            score.valid_kept,
            score.lost_valid,
            score.invalid_caught,
            score.escaped_invalid,
            score.reason_changed,
            score.domain_guarded,
            changed_cases(baseline, run, cases).len(),
        );
    }
    println!("\nBaseline gold mismatches:");
    for (case, actual) in cases.iter().zip(&baseline.outcomes) {
        if !matches_gold(case.label, actual) {
            println!(
                "- `{}`: expected `{}`, actual `{}`",
                case.name,
                expected(case.label),
                normalized(actual),
            );
        }
    }
    println!("\nChanged cases (diagnostic-family sets):");
    for run in runs.iter().skip(1) {
        let changed = changed_cases(baseline, run, cases);
        if changed.is_empty() {
            println!("- `{}`: no observed effect", run.name);
            continue;
        }
        println!("- `{}`:", run.name);
        for name in changed {
            let index = cases.iter().position(|case| case.name == name).unwrap();
            println!(
                "  - `{}`: {} -> {}",
                name,
                normalized(&baseline.outcomes[index]),
                normalized(&run.outcomes[index]),
            );
        }
    }

    println!("\nInteraction contrasts (`11 - 10 - 01 + 00`; `1` means enabled):");
    println!("| interaction | valid kept | lost valid | invalid caught | escaped invalid | reason changed | OOD guarded | OOD unguarded |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for interaction in interactions {
        let contrast = interaction_contrast(interaction, runs);
        println!(
            "| {} × {} | {} | {} | {} | {} | {} | {} | {} |",
            interaction.left,
            interaction.right,
            contrast.valid_kept,
            contrast.lost_valid,
            contrast.invalid_caught,
            contrast.escaped_invalid,
            contrast.reason_changed,
            contrast.domain_guarded,
            contrast.domain_unguarded,
        );
    }
}

fn main() -> Result<(), String> {
    let cases = load_cases()?;
    let baseline = evaluate("baseline", &cases, OwnFeatures::all(), Runtime::Node);
    let mut runs = vec![baseline];
    for ablation in OwnAblation::ALL {
        runs.push(evaluate(
            &format!("no-{}", ablation.slug()),
            &cases,
            OwnFeatures::without(ablation),
            Runtime::Node,
        ));
    }

    let interactions = vec![
        add_interaction(
            &mut runs,
            &cases,
            "function-contracts",
            "local-callee-contracts",
            |features, function_contracts, local_callee_contracts| {
                features.function_contracts = function_contracts;
                features.local_callee_contracts = local_callee_contracts;
            },
        ),
        add_interaction(
            &mut runs,
            &cases,
            "borrow-model",
            "local-borrow-directives",
            |features, borrow_model, local_borrow_directives| {
                features.borrow_model = borrow_model;
                features.local_borrow_directives = local_borrow_directives;
            },
        ),
        add_interaction(
            &mut runs,
            &cases,
            "owned-return-propagation",
            "instance-dispatch",
            |features, owned_return_propagation, instance_dispatch| {
                features.owned_return_propagation = owned_return_propagation;
                features.instance_dispatch = instance_dispatch;
            },
        ),
        add_interaction(
            &mut runs,
            &cases,
            "unknown-call-conservatism",
            "non-consuming-paths",
            |features, unknown_call_conservatism, non_consuming_paths| {
                features.unknown_call_conservatism = unknown_call_conservatism;
                features.non_consuming_paths = non_consuming_paths;
            },
        ),
        add_interaction(
            &mut runs,
            &cases,
            "move-tracking",
            "exact-once",
            |features, move_tracking, exact_once| {
                features.move_tracking = move_tracking;
                features.exact_once = exact_once;
            },
        ),
    ];
    runs.push(evaluate(
        "no-runtime-prelude",
        &cases,
        OwnFeatures::all(),
        Runtime::None,
    ));

    let baseline = &runs[0];
    if env::args().any(|arg| arg == "--csv") {
        print_csv(&runs, &interactions, baseline, &cases);
    } else {
        print_markdown(&runs, &interactions, baseline, &cases);
    }
    Ok(())
}
