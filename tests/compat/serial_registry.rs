//! Guard: every surviving `#[serial]` annotation must justify itself.
//!
//! Serialization is not free — `serial_test`'s unkeyed `#[serial]` puts every
//! annotated test into ONE global lane, so a test that needs no exclusion still
//! blocks every other one. This guard keeps that cost deliberate: each surviving
//! annotation has a row in `tests/SERIAL_REGISTRY.tsv` naming the lane and the
//! reason, and the registry must agree with what the classifier derives from the
//! source. Re-running the classifier is what stops someone writing themselves a
//! row by hand.

use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn registry() -> BTreeMap<String, (String, String)> {
    let text = std::fs::read_to_string(repo_root().join("tests/SERIAL_REGISTRY.tsv"))
        .expect("read tests/SERIAL_REGISTRY.tsv");
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if n == 0 {
            assert_eq!(
                line, "test_fn\tlane\treason",
                "SERIAL_REGISTRY.tsv: unexpected header"
            );
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            3,
            "SERIAL_REGISTRY.tsv line {}: expected 3 tab-separated columns",
            n + 1
        );
        assert!(
            !cols[2].trim().is_empty(),
            "SERIAL_REGISTRY.tsv: {} has an empty reason",
            cols[0]
        );
        let prior = out.insert(
            cols[0].to_string(),
            (cols[1].to_string(), cols[2].to_string()),
        );
        assert!(
            prior.is_none(),
            "SERIAL_REGISTRY.tsv: duplicate row {}",
            cols[0]
        );
    }
    out
}

fn classify() -> BTreeMap<String, String> {
    let out = Command::new("sh")
        .arg(repo_root().join("tests/SERIAL_CLASSIFY.sh"))
        .current_dir(repo_root())
        .output()
        .expect("run tests/SERIAL_CLASSIFY.sh");
    assert!(
        out.status.success(),
        "SERIAL_CLASSIFY.sh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("classifier output is UTF-8");
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (fnname, verdict) = line
            .split_once('\t')
            .expect("classifier emits <fn>\\t<verdict>");
        map.insert(fnname.to_string(), verdict.to_string());
    }
    assert!(!map.is_empty(), "the classifier produced no rows");
    map
}

/// The registry and the classifier must agree, in both directions.
#[test]
fn serial_registry_matches_the_classifier() {
    let reg = registry();
    let derived = classify();
    let expected: BTreeMap<String, String> = derived
        .iter()
        .filter(|(_, v)| v.as_str() != "none")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let missing: Vec<&str> = expected
        .keys()
        .filter(|k| !reg.contains_key(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "these tests are still serialized but have no registry row: {missing:?}"
    );

    let dangling: Vec<&str> = reg
        .keys()
        .filter(|k| !expected.contains_key(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    assert!(
        dangling.is_empty(),
        "these registry rows name nothing that is serialized any more: {dangling:?}"
    );

    let drifted: Vec<String> = expected
        .iter()
        .filter_map(|(k, v)| {
            let (lane, _) = reg.get(k)?;
            (lane != v).then(|| format!("{k}: registry says {lane}, classifier says {v}"))
        })
        .collect();
    assert!(
        drifted.is_empty(),
        "registry/classifier lane drift: {drifted:?}"
    );
}

/// An unkeyed `#[serial]` may only be justified as `global`: it takes the single
/// global lane, so calling it anything narrower misrepresents its cost.
#[test]
fn unkeyed_serial_is_only_ever_global() {
    for (fnname, (lane, _)) in registry() {
        assert!(
            lane == "global" || lane.starts_with("lane:"),
            "{fnname}: lane must be `global` or `lane:<key>`, got {lane}"
        );
    }
}

/// The classifier is a pure function of the tree: two runs agree byte for byte.
#[test]
fn classifier_is_deterministic() {
    assert_eq!(
        classify(),
        classify(),
        "SERIAL_CLASSIFY.sh is not deterministic"
    );
}
