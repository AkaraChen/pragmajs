#[path = "../examples/ablation.rs"]
mod runner;

use runner::{render_csv, run_matrix, Observation};

fn named<'a>(observations: &'a [Observation], name: &str) -> &'a Observation {
    observations
        .iter()
        .find(|observation| observation.name == name)
        .unwrap_or_else(|| panic!("missing matrix cell `{name}`"))
}

#[test]
fn integration_matrix_matches_gold_and_keeps_producers_separate() {
    let observations = run_matrix().expect("integration matrix should run without Corsa");
    assert_eq!(observations.len(), 27, "unexpected matrix size");
    assert!(
        observations
            .iter()
            .all(|observation| observation.matches_gold),
        "gold mismatches: {:#?}",
        observations
            .iter()
            .filter(|observation| !observation.matches_gold)
            .collect::<Vec<_>>()
    );
    assert!(
        observations
            .iter()
            .all(|observation| observation.frontend_parse_count == 1),
        "every cell must reuse one frontend parse"
    );

    let own = named(&observations, "cross-own-off");
    let rt = named(&observations, "cross-rt-off");
    let all = named(&observations, "cross-all-off");
    assert_eq!(
        (own.own_diagnostics.len(), own.rt_diagnostics.len()),
        (1, 0)
    );
    assert_eq!((rt.own_diagnostics.len(), rt.rt_diagnostics.len()), (0, 1));
    assert_eq!(
        (all.own_diagnostics.len(), all.rt_diagnostics.len()),
        (1, 1)
    );

    let compiler = named(&observations, "compiler-own-explicit");
    assert_eq!(compiler.compiler_diagnostics.len(), 1);
    assert!(compiler.rt_diagnostics.is_empty());
    assert!(compiler.provider_errors.is_empty());
    assert!(
        !compiler.combined_failed,
        "the compatibility result intentionally records that own-only ignores TS errors"
    );

    let provider = named(&observations, "provider-all-explicit");
    assert_eq!(provider.provider_errors.len(), 1);
    assert!(provider.compiler_diagnostics.is_empty());
    assert!(provider.provider_errors[0]
        .message
        .contains("deterministic provider failure"));

    let compiler_off = named(&observations, "compiler-rt-off");
    let compiler_explicit = named(&observations, "compiler-rt-explicit");
    assert_eq!(compiler_off.rt_diagnostics.len(), 1);
    assert!(compiler_off.compiler_diagnostics.is_empty());
    assert!(compiler_explicit.rt_diagnostics.is_empty());
    assert_eq!(compiler_explicit.compiler_diagnostics.len(), 1);
}

#[test]
fn matrix_exposes_compiler_evidence_and_independent_platform_axes() {
    let observations = run_matrix().expect("integration matrix should run without Corsa");

    let sparse_off = named(&observations, "sparse-rt-off");
    let sparse_explicit = named(&observations, "sparse-rt-explicit");
    assert_eq!(sparse_off.rt_diagnostics.len(), 2);
    assert!(sparse_explicit.rt_diagnostics.is_empty());

    let own_off = named(&observations, "sparse-own-off");
    let own_explicit = named(&observations, "sparse-own-explicit");
    assert!(own_off.own_diagnostics[0]
        .message
        .starts_with("missing-type:"));
    assert!(own_explicit
        .own_diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.starts_with("missing-type:")));

    let node_node = named(&observations, "platform-node-node");
    let bun_node = named(&observations, "platform-bun-node");
    let node_bun = named(&observations, "platform-node-bun");
    let bun_bun = named(&observations, "platform-bun-bun");
    assert_eq!(node_node.own_diagnostics.len(), 0);
    assert_eq!(bun_node.own_diagnostics.len(), 1);
    assert_eq!(node_bun.own_diagnostics.len(), 0);
    assert_eq!(bun_bun.own_diagnostics.len(), 1);
    assert!(node_node.rt_diagnostics[0]
        .message
        .contains("No static type information"));
    assert!(node_bun.rt_diagnostics.is_empty());
    assert!(bun_bun.rt_diagnostics.is_empty());

    for (name, platform, runtime, target) in [
        ("profile-ecmascript", "ecmascript", "none", "ecmascript"),
        ("profile-browser", "browser", "none", "browser"),
        ("profile-node", "node", "node", "node"),
        ("profile-deno", "deno", "deno", "deno"),
        ("profile-bun", "bun", "bun", "bun"),
    ] {
        let observation = named(&observations, name);
        assert_eq!(observation.platform, platform);
        assert_eq!(observation.runtime, runtime);
        assert_eq!(observation.target, target);
    }

    let profile_bun = named(&observations, "profile-bun");
    assert_eq!(profile_bun.own_diagnostics.len(), 1);
    assert!(profile_bun.own_diagnostics[0]
        .message
        .starts_with("unique-forget:"));
    assert!(profile_bun.rt_diagnostics.is_empty());
}

#[test]
fn unicode_cells_gold_check_scalar_locations_from_every_structured_producer() {
    let observations = run_matrix().expect("integration matrix should run without Corsa");

    let own = named(&observations, "unicode-own-off");
    assert_eq!(
        (own.own_diagnostics[0].line, own.own_diagnostics[0].column),
        (Some(1), Some(75))
    );

    let rt = named(&observations, "unicode-rt-off");
    assert_eq!(
        (rt.rt_diagnostics[0].line, rt.rt_diagnostics[0].column),
        (Some(2), Some(87))
    );

    let compiler = named(&observations, "unicode-compiler-explicit");
    assert_eq!(
        (
            compiler.compiler_diagnostics[0].line,
            compiler.compiler_diagnostics[0].column,
        ),
        (Some(3), Some(10))
    );
}

#[test]
fn csv_is_stable_and_contains_diagnostic_details() {
    let observations = run_matrix().expect("integration matrix should run without Corsa");
    let csv = render_csv(&observations);
    assert_eq!(csv.lines().count(), observations.len() + 1);
    assert!(csv.starts_with("name,fixture,checker,compiler,platform,runtime,target,"));
    assert!(csv.contains("unique-forget:"));
    assert!(csv.contains("Semantic:TS9999: deterministic compiler diagnostic"));
    assert!(csv.contains("Return value of 'incorrectlyPositive'"));
    assert!(
        !csv.contains(env!("CARGO_MANIFEST_DIR")),
        "output should not contain machine-specific absolute paths"
    );
}
