use pragma_own::{check_source_with_features, OwnAblation, OwnFeatures, Runtime};
use std::fs;
use std::path::{Path, PathBuf};

fn corpus() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("ablation");
    let manifest = fs::read_to_string(root.join("manifest.tsv")).unwrap();
    manifest
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (relative, _) = line.split_once('\t').unwrap();
            let path = root.join(relative);
            let source = fs::read_to_string(&path).unwrap();
            Some((path, source))
        })
        .collect()
}

fn outcome(
    path: &Path,
    source: &str,
    features: OwnFeatures,
    runtime: Runtime,
) -> Vec<&'static str> {
    let mut kinds: Vec<_> =
        check_source_with_features(&path.to_string_lossy(), source, runtime, features)
            .kinds()
            .into_iter()
            .map(|kind| kind.slug())
            .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

#[test]
fn every_declared_ownership_ablation_has_an_observed_witness() {
    let cases = corpus();
    let baseline: Vec<_> = cases
        .iter()
        .map(|(path, source)| outcome(path, source, OwnFeatures::all(), Runtime::Node))
        .collect();

    for ablation in OwnAblation::ALL {
        let features = OwnFeatures::without(ablation);
        assert!(
            cases.iter().enumerate().any(|(index, (path, source))| {
                outcome(path, source, features, Runtime::Node) != baseline[index]
            }),
            "no corpus case exercised {}",
            ablation.slug(),
        );
    }
}

#[test]
fn runtime_prelude_ablation_has_an_observed_witness() {
    let cases = corpus();
    assert!(cases.iter().any(|(path, source)| {
        outcome(path, source, OwnFeatures::all(), Runtime::Node)
            != outcome(path, source, OwnFeatures::all(), Runtime::None)
    }));
}

#[test]
fn optional_call_path_ablation_is_callee_sensitive() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("ablation/fixtures/reject-optional-call-then-reuse.js");
    let source = fs::read_to_string(&path).unwrap();

    assert_eq!(
        outcome(&path, &source, OwnFeatures::all(), Runtime::Node),
        vec!["use-after-move"],
        "a definitely bound local function cannot skip its call",
    );
    assert_eq!(
        outcome(
            &path,
            &source,
            OwnFeatures::without(OwnAblation::OptionalCallPaths),
            Runtime::Node,
        ),
        vec!["use-after-move"],
        "ordinary call transfer should expose reuse after the conditional consume",
    );

    let path = root.join("ablation/fixtures/accept-known-optional-call-only.js");
    let source = fs::read_to_string(&path).unwrap();
    assert_eq!(
        outcome(&path, &source, OwnFeatures::all(), Runtime::Node),
        Vec::<&'static str>::new(),
        "the known local consumer should discharge the unique obligation",
    );
    assert_eq!(
        outcome(
            &path,
            &source,
            OwnFeatures::without(OwnAblation::OptionalCallPaths),
            Runtime::Node,
        ),
        Vec::<&'static str>::new(),
        "the ablation should accept a call of a definitely bound local function",
    );

    let path = root.join("ablation/fixtures/reject-unknown-optional-method-only.js");
    let source = fs::read_to_string(&path).unwrap();
    assert_eq!(
        outcome(&path, &source, OwnFeatures::all(), Runtime::Node),
        vec!["unique-forget"],
        "an unknown optional method needs the branch where the call is skipped",
    );
    assert_eq!(
        outcome(
            &path,
            &source,
            OwnFeatures::without(OwnAblation::OptionalCallPaths),
            Runtime::Node,
        ),
        Vec::<&'static str>::new(),
        "treating an unknown optional method as definite must expose the ablation",
    );
}

#[test]
fn local_directive_splits_have_semantic_witnesses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let witnesses = [
        (
            OwnAblation::LocalBorrowDirectives,
            root.join("examples/ok-lifetime-scope.js"),
            &[][..],
            &["unique-forget", "use-after-move"][..],
        ),
        (
            OwnAblation::LocalCloneDirectives,
            root.join("examples/ok-clone.js"),
            &[][..],
            &["use-after-move"][..],
        ),
        (
            OwnAblation::LocalDropDirectives,
            root.join("ablation/fixtures/accept-local-drop-directive.js"),
            &[][..],
            &["unique-forget"][..],
        ),
        (
            OwnAblation::LocalKindDirectives,
            root.join("ablation/fixtures/reject-local-kind-directive.js"),
            &["unique-forget"][..],
            &[][..],
        ),
        (
            OwnAblation::LocalKindDirectives,
            root.join("ablation/fixtures/reject-local-let-directive.js"),
            &["unique-forget"][..],
            &[][..],
        ),
    ];

    for (ablation, path, expected_baseline, expected_ablated) in witnesses {
        let source = fs::read_to_string(&path).unwrap();
        let baseline = outcome(&path, &source, OwnFeatures::all(), Runtime::Node);
        let ablated = outcome(
            &path,
            &source,
            OwnFeatures::without(ablation),
            Runtime::Node,
        );
        assert_eq!(
            baseline,
            expected_baseline,
            "unexpected baseline for {} witness {}",
            ablation.slug(),
            path.display(),
        );
        assert_eq!(
            ablated,
            expected_ablated,
            "unexpected ablated result for {} witness {}",
            ablation.slug(),
            path.display(),
        );
    }
}
