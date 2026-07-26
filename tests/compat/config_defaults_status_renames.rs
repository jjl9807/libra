//! `status.renames` / `status.renameLimit` cascade guards
//! (plan-20260714 R0-7, §B.5).

use super::*;

/// A committed file moved and lightly edited so detection must run the
/// inexact stage (a pure exact pair would mask threshold/limit semantics).
fn staged_inexact_rename_repo(fixture: &Fixture, name: &str) -> PathBuf {
    let repo = fixture.path(name);
    fixture.init_repo(&repo);
    let body: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fixture.commit_file(&repo, "old-name.txt", &body, "base");
    fs::rename(repo.join("old-name.txt"), repo.join("new-name.txt")).expect("rename file");
    fs::write(
        repo.join("new-name.txt"),
        body.replace("line 5\n", "line five\n"),
    )
    .expect("edit renamed file");
    fixture.success(&repo, &["add", "-A"]);
    repo
}

fn status_reports_rename(fixture: &Fixture, repo: &Path) -> bool {
    let out = fixture.success(repo, &["status"]);
    String::from_utf8_lossy(&out.stdout).contains("renamed:")
}

/// `status.renames` cascades local→global→system with local winning, and a
/// `false` at the winning scope splits the rename into delete + add.
#[test]
fn status_renames_cascade_local_beats_global_beats_system() {
    let fixture = Fixture::new();

    // System scope alone disables detection.
    fixture.success(
        Path::new("/"),
        &["config", "--system", "status.renames", "false"],
    );
    let repo = staged_inexact_rename_repo(&fixture, "renames-system");
    assert!(
        !status_reports_rename(&fixture, &repo),
        "system-scope false must disable detection"
    );

    // Global true beats system false.
    fixture.success(
        Path::new("/"),
        &["config", "--global", "status.renames", "true"],
    );
    assert!(
        status_reports_rename(&fixture, &repo),
        "global true beats system false"
    );

    // Local false beats global true.
    fixture.success(&repo, &["config", "status.renames", "false"]);
    assert!(
        !status_reports_rename(&fixture, &repo),
        "local false beats global true"
    );

    // The CLI flag beats every scope.
    let out = fixture.success(&repo, &["status", "--renames"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("renamed:"),
        "--renames overrides the config cascade"
    );
}

/// `status.renames` falls back to `diff.renames` when unset.
#[test]
fn status_renames_falls_back_to_diff_renames() {
    let fixture = Fixture::new();
    let repo = staged_inexact_rename_repo(&fixture, "renames-fallback");
    fixture.success(&repo, &["config", "diff.renames", "false"]);
    assert!(
        !status_reports_rename(&fixture, &repo),
        "diff.renames=false is inherited"
    );
    fixture.success(&repo, &["config", "status.renames", "true"]);
    assert!(
        status_reports_rename(&fixture, &repo),
        "status.renames beats the diff fallback"
    );
}

/// `copy`/`copies` values fail closed (R0 has no copy detection) and invalid
/// booleans fail closed too — never a silent downgrade.
#[test]
fn status_renames_copy_and_invalid_fail_closed() {
    let fixture = Fixture::new();
    let repo = staged_inexact_rename_repo(&fixture, "renames-copy");

    fixture.success(&repo, &["config", "status.renames", "copy"]);
    let out = fixture.run(&repo, &["status"]);
    assert!(!out.status.success(), "copy must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("copy detection is not supported"),
        "copy failure is actionable: {stderr}"
    );

    fixture.success(&repo, &["config", "status.renames", "sideways"]);
    let out = fixture.run(&repo, &["status"]);
    assert!(!out.status.success(), "invalid boolean must fail closed");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("status.renames"),
        "failure names the key"
    );
}

/// `status.renameLimit` inherits `diff.renameLimit` across scopes and rejects
/// invalid values before any output.
#[test]
fn status_rename_limit_cascade_and_invalid_fail_closed() {
    let fixture = Fixture::new();
    let repo = staged_inexact_rename_repo(&fixture, "rename-limit");

    // A GLOBAL diff.renameLimit is inherited when the status key is unset.
    // The staged new side holds TWO candidates (.libraignore + the renamed
    // file), so limit=1 gates the exhaustive stage with the warning…
    fixture.success(
        Path::new("/"),
        &["config", "--global", "diff.renameLimit", "1"],
    );
    let out = fixture.success(&repo, &["status"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("renamed:"),
        "inherited limit=1 gates the 2-candidate exhaustive stage"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("renameLimit"),
        "the degradation warning names the limit"
    );

    // …while a local status.renameLimit=2 wins over the fallback and lets
    // the pair through again.
    fixture.success(&repo, &["config", "status.renameLimit", "2"]);
    assert!(
        status_reports_rename(&fixture, &repo),
        "status.renameLimit beats the diff fallback"
    );

    // …and an invalid status.renameLimit still fails closed first.
    fixture.success(&repo, &["config", "status.renameLimit", "many"]);
    let out = fixture.run(&repo, &["status"]);
    assert!(
        !out.status.success(),
        "invalid renameLimit must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("status.renameLimit"),
        "failure names the key"
    );
}
