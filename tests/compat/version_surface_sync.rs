//! plan-20260714 PD-00 follow-up guard: the FOUR release version surfaces
//! must stay in lockstep.
//!
//! PD-00 closed a drift where `Cargo.toml`, `web/package.json` and
//! `worker/package.json` were bumped but `install.sh`'s `DEFAULT_VERSION`
//! fallback was left behind — a stale fallback silently installs an OLD
//! binary whenever the release API is unreachable and
//! `LIBRA_ALLOW_FALLBACK=1` is set. The card's explicit follow-up
//! condition ("if it drifts again, add a guard that reads all four files
//! and asserts they agree") is what this target implements.

use std::{fs, path::Path};

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn cargo_version() -> String {
    let text = read("Cargo.toml");
    // The workspace package version is the first `version = "…"` under
    // `[package]`, which is the first table in this manifest.
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("version = \"")
            && let Some(value) = rest.strip_suffix('"')
        {
            return value.to_string();
        }
    }
    panic!("Cargo.toml has no package version line");
}

fn package_json_version(relative: &str) -> String {
    let text = read(relative);
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("\"version\": \"")
            && let Some(value) = rest.strip_suffix("\",").or_else(|| rest.strip_suffix('"'))
        {
            return value.to_string();
        }
    }
    panic!("{relative} has no version field");
}

fn install_default_version() -> String {
    let text = read("install.sh");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("DEFAULT_VERSION=\"")
            && let Some(value) = rest.strip_suffix('"')
        {
            // The shell fallback carries the `v` release-tag prefix.
            return value.trim_start_matches('v').to_string();
        }
    }
    panic!("install.sh has no DEFAULT_VERSION assignment");
}

#[test]
fn all_four_release_version_surfaces_agree() {
    let cargo = cargo_version();
    let web = package_json_version("web/package.json");
    let worker = package_json_version("worker/package.json");
    let install = install_default_version();

    assert_eq!(
        web, cargo,
        "web/package.json version must match Cargo.toml ({cargo})"
    );
    assert_eq!(
        worker, cargo,
        "worker/package.json version must match Cargo.toml ({cargo})"
    );
    assert_eq!(
        install, cargo,
        "install.sh DEFAULT_VERSION must match Cargo.toml ({cargo}) — a stale \
         fallback installs an old binary when the release API is unreachable"
    );
}

#[test]
fn install_default_version_carries_the_release_tag_prefix() {
    let text = read("install.sh");
    let line = text
        .lines()
        .find(|line| line.starts_with("DEFAULT_VERSION="))
        .expect("install.sh DEFAULT_VERSION");
    assert!(
        line.contains("=\"v"),
        "DEFAULT_VERSION keeps the `v<semver>` release-tag spelling: {line}"
    );
}
