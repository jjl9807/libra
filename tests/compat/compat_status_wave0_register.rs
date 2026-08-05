//! Registration gate for the wave-0 status test module (plan-20260714 §B.9).
//!
//! Guards against two silent-drop failure modes:
//! 1. `tests/command/status_wave0_test.rs` exists but is not wired into
//!    `tests/command/mod.rs` (CI would silently skip every wave-0 test).
//! 2. The canonical manifest (`STATUS_WAVE0_TESTS`) drifts from the module
//!    contents in either direction.
//!
//! This target must not spawn `cargo test --test command_test -- --list` to
//! discover registrations: Cargo holds the target-directory lock while this
//! test runs, so that child Cargo process waits on its parent indefinitely.
//! The source-level parser below verifies the same module wiring and test-name
//! contract without recursively invoking Cargo.

use std::{collections::HashSet, fs, path::Path};

#[path = "status_wave0_manifest.rs"]
mod status_wave0_manifest;

use status_wave0_manifest::{STATUS_WAVE0_TESTS, STATUS_WAVE0_TESTS_UNIX_ONLY};

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// Return the names of direct `#[test]`/`#[tokio::test]` functions declared
/// by the Wave-0 module. The module deliberately keeps this simple shape; if
/// it needs a macro-generated test in future, extend this parser and its test
/// alongside that change rather than silently dropping it from the manifest.
fn declared_wave0_tests(source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut pending_test_attribute = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed == "#[test]" || trimmed.starts_with("#[tokio::test") {
            pending_test_attribute = true;
            continue;
        }
        if !pending_test_attribute {
            continue;
        }
        // Test attributes such as `#[serial]` and conditional compilation can
        // appear between the test marker and its function declaration.
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
            continue;
        }

        let declaration = trimmed
            .strip_prefix("pub ")
            .unwrap_or(trimmed)
            .strip_prefix("async fn ")
            .or_else(|| trimmed.strip_prefix("fn "));
        let Some(declaration) = declaration else {
            panic!(
                "expected a function declaration after a Wave-0 test attribute, found: {trimmed}"
            );
        };
        let name = declaration
            .split(|character: char| {
                character == '(' || character == '<' || character.is_whitespace()
            })
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| panic!("test declaration has no function name: {trimmed}"));
        assert!(
            names.insert(name.to_string()),
            "Wave-0 source declares duplicate test function `{name}`"
        );
        pending_test_attribute = false;
    }

    assert!(
        !pending_test_attribute,
        "Wave-0 source ends after a test attribute without a function declaration"
    );
    names
}

#[test]
fn status_wave0_manifest_matches_registered_tests() {
    let command_module = read("tests/command/mod.rs");
    assert!(
        command_module.contains("#[path = \"status_wave0_test.rs\"]\nmod status_wave0;"),
        "tests/command/status_wave0_test.rs must be registered by tests/command/mod.rs"
    );
    let actual = declared_wave0_tests(&read("tests/command/status_wave0_test.rs"));

    let manifest: HashSet<&str> = STATUS_WAVE0_TESTS.iter().copied().collect();
    assert_eq!(
        STATUS_WAVE0_TESTS.len(),
        manifest.len(),
        "STATUS_WAVE0_TESTS contains duplicate names"
    );

    let unix_only: HashSet<&str> = STATUS_WAVE0_TESTS_UNIX_ONLY.iter().copied().collect();
    assert_eq!(
        STATUS_WAVE0_TESTS_UNIX_ONLY.len(),
        unix_only.len(),
        "STATUS_WAVE0_TESTS_UNIX_ONLY contains duplicate names"
    );
    assert!(
        unix_only.is_subset(&manifest),
        "STATUS_WAVE0_TESTS_UNIX_ONLY must be a subset of STATUS_WAVE0_TESTS"
    );

    // Parse source rather than Cargo's host-specific test listing: the
    // manifest is canonical across platforms, including `#[cfg(unix)]`
    // functions that a Windows binary intentionally does not compile.
    let expected: HashSet<String> = STATUS_WAVE0_TESTS
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    assert!(
        !expected.is_empty(),
        "STATUS_WAVE0_TESTS must not be empty — the wave-0 module would be silently dropped"
    );
    assert_eq!(
        expected, actual,
        "STATUS_WAVE0_TESTS and tests/command/status_wave0_test.rs drifted; \
         update tests/compat/status_wave0_manifest.rs together with the module"
    );
}

#[test]
fn status_wave0_manifest_is_strictly_sorted() {
    assert!(
        STATUS_WAVE0_TESTS.windows(2).all(|w| w[0] < w[1]),
        "STATUS_WAVE0_TESTS must be strictly alphabetically sorted with no duplicates"
    );
    assert!(
        STATUS_WAVE0_TESTS_UNIX_ONLY.windows(2).all(|w| w[0] < w[1]),
        "STATUS_WAVE0_TESTS_UNIX_ONLY must be strictly alphabetically sorted with no duplicates"
    );
}
