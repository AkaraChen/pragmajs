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
