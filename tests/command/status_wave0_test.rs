//! Wave-0 status regressions (plan-20260714 Part B).
//!
//! The canonical list of tests in this module lives in
//! `tests/compat/status_wave0_manifest.rs` (`STATUS_WAVE0_TESTS`); the
//! `compat_status_wave0_register` gate asserts the two stay in sync in both
//! directions. Rename-detection behavior tests join this module slice by
//! slice as R0-1..R0-8 land (see plan B.8/B.9).

use std::{fs, path::Path};

use git_internal::internal::{
    index::{Index, IndexEntry},
    object::blob::Blob,
};
use libra::{command::save_object, utils::path};

use super::*;

fn create_repo_with_committed_file(path: &str, content: &str) -> tempfile::TempDir {
    let repo = tempdir().expect("failed to create temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::write(repo.path().join(path), content).expect("failed to write committed fixture");

    let add = run_libra_command(&["add", path], repo.path());
    assert_cli_success(&add, "stage committed fixture");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit fixture");
    repo
}

fn status_stdout(repo: &Path, args: &[&str]) -> String {
    let output = run_libra_command(args, repo);
    assert_cli_success(&output, "status command");
    String::from_utf8(output.stdout).expect("status stdout should be utf-8")
}

fn write_blob_to_repo(content: &str) -> (ObjectHash, u32) {
    let blob = Blob::from_content(content);
    save_object(&blob, &blob.id).expect("failed to save blob");
    (blob.id, blob.data.len() as u32)
}

fn add_index_stage(index: &mut Index, file: &str, content: &str, stage: u8) {
    let (hash, size) = write_blob_to_repo(content);
    let mut entry = IndexEntry::new_from_blob(file.to_string(), hash, size);
    entry.flags.stage = stage;
    index.add(entry);
}

#[test]
#[serial]
fn porcelain_v2_unmerged_u_line() {
    let repo = create_repo_with_committed_file("conflict.txt", "base\n");
    let _guard = ChangeDirGuard::new(repo.path());
    let mut index = Index::new();
    add_index_stage(&mut index, "conflict.txt", "base\n", 1);
    add_index_stage(&mut index, "conflict.txt", "ours\n", 2);
    add_index_stage(&mut index, "conflict.txt", "theirs\n", 3);
    index
        .save(path::index())
        .expect("failed to write unmerged index");

    let output = status_stdout(repo.path(), &["status", "--porcelain", "v2"]);
    let u_line = output
        .lines()
        .find(|line| line.starts_with("u UU "))
        .expect("expected porcelain v2 unmerged row");
    let fields: Vec<_> = u_line.split_whitespace().collect();

    assert_eq!(fields.len(), 11, "unexpected u-line fields: {u_line}");
    assert_eq!(fields[1], "UU");
    assert_eq!(fields[10], "conflict.txt");
}

#[test]
#[serial]
fn resolved_conflict_with_stage0_emits_no_u_line() {
    let repo = create_repo_with_committed_file("conflict.txt", "base\n");
    let _guard = ChangeDirGuard::new(repo.path());
    let mut index = Index::new();
    add_index_stage(&mut index, "conflict.txt", "base\n", 1);
    add_index_stage(&mut index, "conflict.txt", "ours\n", 2);
    add_index_stage(&mut index, "conflict.txt", "theirs\n", 3);
    add_index_stage(&mut index, "conflict.txt", "resolved\n", 0);
    index
        .save(path::index())
        .expect("failed to write resolved index");

    let output = status_stdout(repo.path(), &["status", "--porcelain", "v2"]);

    assert!(
        !output.lines().any(|line| line.starts_with("u ")),
        "resolved stage-0 path must not emit u line:\n{output}"
    );
    assert!(
        output.lines().any(|line| line.starts_with("1 M")),
        "resolved stage-0 path should be rendered as a normal tracked row:\n{output}"
    );
}

#[test]
#[serial]
fn unmerged_stage_presence_to_xy_mapping() {
    // Exercises the seven Git unmerged stage-presence combinations through the
    // public `--short` surface (stage 1 = base, 2 = ours, 3 = theirs).
    let cases = [
        ((false, true, true), "AA"),
        ((true, false, false), "DD"),
        ((false, true, false), "AU"),
        ((false, false, true), "UA"),
        ((true, false, true), "DU"),
        ((true, true, false), "UD"),
        ((true, true, true), "UU"),
    ];

    for ((base, ours, theirs), expected) in cases {
        let repo = create_repo_with_committed_file("conflict.txt", "base\n");
        let _guard = ChangeDirGuard::new(repo.path());
        let mut index = Index::new();
        if base {
            add_index_stage(&mut index, "conflict.txt", "base\n", 1);
        }
        if ours {
            add_index_stage(&mut index, "conflict.txt", "ours\n", 2);
        }
        if theirs {
            add_index_stage(&mut index, "conflict.txt", "theirs\n", 3);
        }
        index
            .save(path::index())
            .expect("failed to write unmerged index");

        let output = status_stdout(repo.path(), &["status", "--short"]);
        assert!(
            output
                .lines()
                .any(|line| line.starts_with(expected) && line.ends_with("conflict.txt")),
            "expected XY {expected} for base={base} ours={ours} theirs={theirs}, got:\n{output}"
        );
    }
}

#[test]
fn porcelain_v1_rename_output_stays_add_delete() {
    let staged = libra::command::status::Changes {
        new: vec!["b.txt".into()],
        modified: vec![],
        deleted: vec!["a.txt".into()],
        renamed: vec![],
    };
    let unstaged = libra::command::status::Changes::default();
    let mut output = Vec::new();

    libra::command::status::output_porcelain(&staged, &unstaged, false, &mut output)
        .expect("porcelain v1 output should succeed");

    let rendered = String::from_utf8(output).expect("porcelain v1 should be utf-8");
    assert_eq!(rendered, "D  a.txt\nA  b.txt\n");
}

/// `--porcelain` (v1) renders a detected rename as a single `R  old -> new`
/// record, not two `R` endpoint rows (§B.6.3).
#[test]
fn porcelain_v1_uses_rename_arrow_when_detected() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        out.lines().any(|l| l == "R  a.txt -> b.txt"),
        "porcelain v1 rename should be a single arrow record: {out:?}"
    );
    assert!(
        !out.lines().any(|l| l == "R  a.txt" || l == "R  b.txt"),
        "endpoints must not double as separate R rows: {out:?}"
    );
}

// ── R0-2/R0-4: engine-backed rename detection, default-on (§B.4/§B.5) ─────────

/// A staged move of unchanged content is an exact rename, detected by default
/// (rename detection is ON without any flag, matching Git).
#[test]
fn rename_exact_staged_detected_by_default() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("a.txt") && out.contains("b.txt"),
        "default status should report the rename: {out}"
    );
    // The endpoints must NOT also appear as a separate delete + new file.
    assert!(
        !out.contains("deleted: a.txt") && !out.contains("new file: b.txt"),
        "rename endpoints must not double as add/delete: {out}"
    );
}

/// A staged move with a small content edit is still a rename (inexact,
/// spanhash similarity above the 50% default threshold).
#[test]
fn rename_inexact_content_change_detected() {
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    // Edit one line of the moved file, then re-stage it.
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("moved.txt"), edited).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited moved file");

    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("orig.txt") && out.contains("moved.txt"),
        "inexact rename should still be detected: {out}"
    );
}

/// `--no-renames` disables detection, so the same move renders as a delete +
/// add pair.
#[test]
fn rename_no_renames_flag_splits_add_delete() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status", "--no-renames"]);
    assert!(
        out.contains("deleted:") && out.contains("a.txt") && out.contains("b.txt"),
        "--no-renames should split into delete + new file: {out}"
    );
    assert!(
        !out.contains("renamed:"),
        "--no-renames must not report a rename: {out}"
    );
}

/// `status.renames=false` disables detection through the config cascade,
/// even though the feature default is on (§B.5).
#[test]
fn rename_config_status_renames_false_disables() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let cfg = run_libra_command(&["config", "status.renames", "false"], repo.path());
    assert_cli_success(&cfg, "set status.renames=false");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        !out.contains("renamed:") && out.contains("deleted:"),
        "status.renames=false should disable rename detection: {out}"
    );
}

/// A CLI `--find-renames` always wins over a config `status.renames=false`.
#[test]
fn rename_config_cli_find_renames_overrides_false() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let cfg = run_libra_command(&["config", "status.renames", "false"], repo.path());
    assert_cli_success(&cfg, "set status.renames=false");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status", "--find-renames"]);
    assert!(
        out.contains("renamed:"),
        "--find-renames must override status.renames=false: {out}"
    );
}

/// `--short` renders a detected rename as a single Git-style `R  old -> new`
/// line, not two separate `R` rows (§B.6.1).
#[test]
fn rename_short_format_uses_arrow() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    // Force no color so the line is the plain `R  a.txt -> b.txt` form.
    let out = status_stdout(repo.path(), &["--no-color", "status", "--short"]);
    assert!(
        out.lines().any(|l| l.contains("a.txt -> b.txt")),
        "short rename should use the arrow form: {out}"
    );
    // The endpoints must not also appear as two separate `R` rows.
    assert!(
        !out.lines().any(|l| l.trim_end() == "R  a.txt"),
        "rename endpoints must not double as separate R rows: {out}"
    );
}

/// A tracked file that cannot be read (permission denied on its parent) must
/// NOT be reported as deleted — status fails closed instead (§B.6.0.1). This
/// prevents `commit -a` from recording a spurious removal.
#[test]
#[cfg(unix)]
fn tracked_unreadable_path_fails_closed_not_deleted() {
    use std::os::unix::fs::PermissionsExt;
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::create_dir(repo.path().join("locked")).unwrap();
    fs::write(repo.path().join("locked/secret.txt"), "top secret\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "locked/secret.txt"], repo.path()),
        "stage tracked file",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path()),
        "commit tracked file",
    );

    // Make the parent directory unreadable/untraversable so symlink_metadata
    // on the tracked file returns EACCES rather than NotFound.
    let dir = repo.path().join("locked");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    let output = run_libra_command(&["status"], repo.path());
    // Restore permissions before asserting so the tempdir can be cleaned up.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        !output.status.success(),
        "unreadable tracked path must fail closed, not succeed with a deletion"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LBR-IO-001") || stderr.contains("cannot read tracked path"),
        "fails closed with an IO error: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("deleted:") && !stdout.contains("secret.txt"),
        "must not report the unreadable file as deleted: {stdout}"
    );
}

/// `--porcelain=v2` emits Git's single `2 R<score> …\t<old>` rename record
/// (real HEAD/index modes and hashes), not two `1 R` change rows (§B.6.4).
#[test]
fn rename_porcelain_v2_emits_rename_record() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let line = out
        .lines()
        .find(|l| l.starts_with("2 "))
        .unwrap_or_else(|| panic!("expected a porcelain v2 rename record: {out}"));
    let fields: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(fields[0], "2", "{line}");
    assert_eq!(fields[1], "R.", "staged rename xy: {line}");
    // Score field is `R<digits>` at index 8 (NOT the xy `R.` at index 1).
    let score = fields[8];
    assert!(
        score.starts_with('R') && score[1..].chars().all(|c| c.is_ascii_digit()),
        "score field is R<pct>: {line}"
    );
    assert_eq!(&score[1..], "100", "exact rename scores 100: {line}");
    // HEAD/index hashes must be real (non-zero) for an exact staged rename.
    assert_ne!(
        fields[6],
        "0".repeat(fields[6].len()),
        "hH non-zero: {line}"
    );
    assert_ne!(
        fields[7],
        "0".repeat(fields[7].len()),
        "hI non-zero: {line}"
    );
    // The record carries both paths (new\told); endpoints must not also
    // appear as separate `1 R` rows.
    assert!(line.contains("b.txt") && line.contains("a.txt"), "{line}");
    assert!(
        !out.lines().any(|l| l.starts_with("1 R")),
        "endpoints must not double as `1 R` rows: {out}"
    );
}

/// `--json` includes a top-level `renames[]` array with `score`, `exact`, and
/// side flags (§B.6.5).
#[test]
fn rename_json_includes_score_and_side() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json status");
    let renames = doc["data"]["renames"]
        .as_array()
        .expect("renames array present");
    let entry = renames
        .iter()
        .find(|r| r["to"] == "b.txt")
        .unwrap_or_else(|| panic!("rename entry for b.txt: {out}"));
    assert_eq!(entry["from"], "a.txt");
    assert_eq!(entry["score"], 100);
    assert_eq!(entry["exact"], true);
    assert_eq!(entry["staged"], true);
    assert_eq!(entry["unstaged"], false);
}

/// Detection runs on repo-relative keys, so a rename is found even when
/// `status` is invoked from a subdirectory (the historical subdir bug).
#[test]
fn rename_from_subdirectory_detected() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::create_dir(repo.path().join("sub")).unwrap();
    fs::write(
        repo.path().join("sub/a.txt"),
        "subdir rename content\nsecond line here\n",
    )
    .unwrap();
    let add = run_libra_command(&["add", "sub/a.txt"], repo.path());
    assert_cli_success(&add, "stage subdir file");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit subdir file");
    let mv = run_libra_command(&["mv", "sub/a.txt", "sub/b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv in subdir");

    // Invoke status FROM the subdirectory.
    let out = status_stdout(&repo.path().join("sub"), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("a.txt") && out.contains("b.txt"),
        "rename must be detected from a subdirectory: {out}"
    );
}

// ── §B.3.1: untracked paths as rename destinations (Libra extension) ─────────

/// Default (`status.renameUntracked` unset = false, Git parity): a chain of
/// staged `a→b` plus an unstaged worktree move `b→c` reports the staged
/// rename normally, but the second hop stays `D` + `??` — the untracked
/// destination is NOT consumed into an unstaged rename record.
#[test]
fn chain_rename_default_untracked_d_and_question() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv (staged hop)");
    // Unstaged second hop: move b.txt → c.txt purely in the worktree.
    let contents = fs::read(repo.path().join("b.txt")).expect("read staged move target");
    fs::remove_file(repo.path().join("b.txt")).expect("remove worktree b.txt");
    fs::write(repo.path().join("c.txt"), contents).expect("write untracked c.txt");

    let out = status_stdout(repo.path(), &["status", "--porcelain"]);
    // Git parity: the worktree deletion of the rename destination rides in
    // the Y column of the rename record (`RD`), not as a separate ` D` row.
    assert!(
        out.lines().any(|l| l == "RD a.txt -> b.txt"),
        "staged rename record carries the worktree delete as Y=D: {out:?}"
    );
    assert!(
        out.lines().any(|l| l == "?? c.txt"),
        "unstaged hop destination stays untracked: {out:?}"
    );
    assert!(
        !out.lines().any(|l| l.contains("b.txt -> c.txt")),
        "no unstaged rename record without status.renameUntracked: {out:?}"
    );

    // Porcelain v2: xy = RD and mW = 000000 (no worktree entry for the
    // deleted destination — must not fabricate 100644).
    let v2 = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let record = v2
        .lines()
        .find(|l| l.starts_with("2 "))
        .unwrap_or_else(|| panic!("expected v2 rename record: {v2}"));
    let fields: Vec<&str> = record.split_whitespace().collect();
    assert_eq!(
        fields[1], "RD",
        "v2 xy carries the worktree delete: {record}"
    );
    assert_eq!(
        fields[5], "000000",
        "mW is zero for a deleted destination: {record}"
    );

    // `-z` v1: `RD SP <new> NUL <old> NUL` record shape.
    let z = run_libra_command(&["status", "--porcelain", "-z"], repo.path());
    assert!(z.status.success());
    let raw = String::from_utf8_lossy(&z.stdout);
    assert!(
        raw.contains("RD b.txt\0a.txt\0"),
        "-z record keeps the RD xy with NUL-separated new/old: {raw:?}"
    );
}

/// A staged rename whose destination is then modified in the worktree emits
/// a single `RM old -> new` record (§B.9 `staged_rename_then_modify_emits_rm`)
/// in short and porcelain v1, and `R.`→`RM` in the v2 xy field.
#[test]
fn staged_rename_then_modify_emits_rm() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    fs::write(
        repo.path().join("b.txt"),
        "hello rename world\ncontent line two\nworktree edit\n",
    )
    .expect("modify rename destination in worktree");

    let v1 = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        v1.lines().any(|l| l == "RM a.txt -> b.txt"),
        "porcelain v1 merges the worktree edit into RM: {v1:?}"
    );
    let short = status_stdout(repo.path(), &["--no-color", "status", "--short"]);
    assert!(
        short.lines().any(|l| l.starts_with("RM ")),
        "short format carries Y=M on the rename line: {short:?}"
    );
    let v2 = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let record = v2
        .lines()
        .find(|l| l.starts_with("2 "))
        .unwrap_or_else(|| panic!("expected v2 rename record: {v2}"));
    assert!(
        record.split_whitespace().nth(1) == Some("RM"),
        "v2 xy is RM when the destination has a worktree edit: {record}"
    );
}

/// `status.renameUntracked` is a strict-bool config cascade: enabling it (in
/// either scope, local overriding global) lets the same worktree move pair
/// into an unstaged rename, and an invalid value fails closed before output.
#[test]
fn rename_untracked_config_cascade() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    fs::remove_file(repo.path().join("a.txt")).expect("remove worktree a.txt");
    fs::write(
        repo.path().join("moved.txt"),
        "hello rename world\ncontent line two\n",
    )
    .expect("write untracked moved.txt");

    // Default off: D + ??.
    let off = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        off.lines().any(|l| l == " D a.txt") && off.lines().any(|l| l == "?? moved.txt"),
        "default keeps D + ??: {off:?}"
    );

    // Global scope enables it (numeric Git boolean exercises strict parsing).
    let global = run_libra_command(
        &["config", "set", "--global", "status.renameUntracked", "1"],
        repo.path(),
    );
    assert_cli_success(&global, "set global status.renameUntracked=1");
    let on = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        on.lines().any(|l| l == " R a.txt -> moved.txt"),
        "global true enables the unstaged rename pair: {on:?}"
    );

    // Local false overrides global true (cascade order).
    let local = run_libra_command(
        &["config", "set", "status.renameUntracked", "false"],
        repo.path(),
    );
    assert_cli_success(&local, "set local status.renameUntracked=false");
    let overridden = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        overridden.lines().any(|l| l == " D a.txt"),
        "local false wins over global true: {overridden:?}"
    );

    // Invalid value fails closed before any status output.
    let invalid = run_libra_command(
        &["config", "set", "status.renameUntracked", "sideways"],
        repo.path(),
    );
    assert_cli_success(&invalid, "store invalid value");
    let failed = run_libra_command(&["status", "--porcelain"], repo.path());
    assert!(
        !failed.status.success(),
        "invalid status.renameUntracked must fail closed"
    );
    assert!(
        failed.stdout.is_empty(),
        "no partial porcelain before the config failure: {:?}",
        String::from_utf8_lossy(&failed.stdout)
    );
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.contains("status.renameUntracked"),
        "diagnostic names the offending key: {stderr}"
    );
}

/// Per-end pathspec semantics (§B.3): a rename record survives only when
/// BOTH endpoints match the pathspec. An old-only match reports the in-scope
/// deletion, a new-only match the in-scope addition — an out-of-scope
/// endpoint never leaks through a rename record (default staged path).
#[test]
fn pathspec_old_only_new_only_matrix() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    // Old endpoint only: the deletion is in scope, the rename is not.
    let old_only = status_stdout(repo.path(), &["status", "--porcelain=v1", "a.txt"]);
    assert!(
        old_only.lines().any(|l| l == "D  a.txt"),
        "old-only pathspec demotes to a deletion: {old_only:?}"
    );
    assert!(
        !old_only.contains("b.txt"),
        "out-of-scope destination must not leak: {old_only:?}"
    );

    // New endpoint only: the addition is in scope, the rename is not.
    let new_only = status_stdout(repo.path(), &["status", "--porcelain=v1", "b.txt"]);
    assert!(
        new_only.lines().any(|l| l == "A  b.txt"),
        "new-only pathspec demotes to an addition: {new_only:?}"
    );
    assert!(
        !new_only.contains("a.txt"),
        "out-of-scope source must not leak: {new_only:?}"
    );

    // Both endpoints in scope: the rename record is kept.
    let both = status_stdout(repo.path(), &["status", "--porcelain=v1", "a.txt", "b.txt"]);
    assert!(
        both.lines().any(|l| l == "R  a.txt -> b.txt"),
        "both endpoints in scope keep the rename record: {both:?}"
    );
}

/// A staged rename whose destination is then DELETED in the worktree emits a
/// single `RD old -> new` record in short and porcelain v1, and `RD` with
/// `mW=000000` in v2 (§B.9 `staged_rename_then_delete_emits_rd`).
#[test]
fn staged_rename_then_delete_emits_rd() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    fs::remove_file(repo.path().join("b.txt")).expect("delete rename destination in worktree");

    let short = status_stdout(repo.path(), &["--no-color", "status", "--short"]);
    assert!(
        short.lines().any(|l| l == "RD a.txt -> b.txt"),
        "short format carries Y=D on the rename line: {short:?}"
    );
    let v1 = status_stdout(repo.path(), &["status", "--porcelain"]);
    assert!(
        v1.lines().any(|l| l == "RD a.txt -> b.txt"),
        "porcelain v1 merges the worktree delete into RD: {v1:?}"
    );
    let v2 = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let record = v2
        .lines()
        .find(|l| l.starts_with("2 "))
        .unwrap_or_else(|| panic!("expected v2 rename record: {v2}"));
    let fields: Vec<&str> = record.split_whitespace().collect();
    assert_eq!(
        fields[1], "RD",
        "v2 xy carries the worktree delete: {record}"
    );
    assert_eq!(
        fields[5], "000000",
        "mW is zero for a deleted destination: {record}"
    );
}

// ── R0-8a: structured warnings + 9≻1 exit arbitration (§B.5) ────────────────

/// Serialization names of the warning schema are pinned (§B.5): `code` and
/// `source` serialize in snake_case exactly as documented.
#[test]
fn json_warnings_schema_snapshot() {
    let warning = libra::command::status::StatusWarning {
        code: libra::command::status::StatusWarningCode::RenameLimitProductSkipped,
        message: "m".to_string(),
        source: libra::command::status::StatusWarningSource::RenameDetect,
    };
    let value = serde_json::to_value(&warning).expect("serialize warning");
    assert_eq!(
        value,
        serde_json::json!({
            "code": "rename_limit_product_skipped",
            "message": "m",
            "source": "rename_detect",
        }),
        "full object shape is pinned"
    );
    let budget =
        serde_json::to_value(libra::command::status::StatusWarningCode::SimilarityBudgetExceeded)
            .expect("serialize code");
    assert_eq!(budget, "similarity_budget_exceeded");
    for (code, name) in [
        (
            libra::command::status::StatusWarningCode::DirtyCacheLockStolen,
            "dirty_cache_lock_stolen",
        ),
        (
            libra::command::status::StatusWarningCode::DirtyCacheStaleFallback,
            "dirty_cache_stale_fallback",
        ),
        (
            libra::command::status::StatusWarningCode::DirtyCacheConcurrentInvalidate,
            "dirty_cache_concurrent_invalidate",
        ),
    ] {
        assert_eq!(serde_json::to_value(code).expect("serialize code"), name);
    }
    assert_eq!(
        serde_json::to_value(libra::command::status::StatusWarningSource::Cache).expect("src"),
        "cache"
    );
}

/// Exceeding the per-side rename limit (1000) degrades the inexact pass with
/// a structured warning; with `--exit-code-on-warning` the warning exit 9
/// beats the `--exit-code` dirty exit 1 in text AND JSON modes, and JSON
/// carries the warning in `data.warnings[]` with no stderr line.
#[test]
fn rename_limit_warning_exit_nine_over_dirty() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    for i in 0..1001 {
        fs::write(repo.path().join(format!("f{i}.txt")), format!("base {i}\n")).unwrap();
    }
    let add = run_libra_command(&["add", "."], repo.path());
    assert_cli_success(&add, "stage base files");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit base files");
    for i in 0..1001 {
        fs::remove_file(repo.path().join(format!("f{i}.txt"))).unwrap();
        fs::write(
            repo.path().join(format!("g{i}.txt")),
            format!("completely different payload {i}\n"),
        )
        .unwrap();
    }
    let add = run_libra_command(&["add", "."], repo.path());
    assert_cli_success(&add, "stage mass rename");

    // Text mode: warning on stderr; exit 9 beats dirty 1.
    let text = run_libra_command(
        &["--exit-code-on-warning", "status", "--exit-code"],
        repo.path(),
    );
    assert_eq!(
        text.status.code(),
        Some(9),
        "text: warning exit 9 over dirty 1"
    );
    let stderr = String::from_utf8_lossy(&text.stderr);
    assert!(
        stderr.contains("warning:") && stderr.contains("renameLimit"),
        "text stderr carries the structured warning: {stderr}"
    );

    // JSON mode: warnings ride in data.warnings[], stderr stays clean, and
    // the same silent exit 9 wins.
    let json = run_libra_command(
        &["--json", "--exit-code-on-warning", "status", "--exit-code"],
        repo.path(),
    );
    assert_eq!(
        json.status.code(),
        Some(9),
        "json: warning exit 9 over dirty 1"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&json.stdout)).expect("json envelope");
    let warnings = doc["data"]["warnings"].as_array().expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "rename_limit_product_skipped"),
        "json data.warnings carries the code: {doc}"
    );
    assert!(
        json.stderr.is_empty(),
        "json mode keeps stderr completely empty: {:?}",
        String::from_utf8_lossy(&json.stderr)
    );

    // Without --exit-code-on-warning the dirty exit 1 still applies.
    let plain = run_libra_command(&["status", "--exit-code"], repo.path());
    assert_eq!(
        plain.status.code(),
        Some(1),
        "dirty exit stays 1 without on-warning"
    );

    // --quiet suppresses the body but never the diagnostics; 9 still wins.
    let quiet = run_libra_command(
        &["--quiet", "--exit-code-on-warning", "status", "--exit-code"],
        repo.path(),
    );
    assert_eq!(quiet.status.code(), Some(9), "quiet: warning exit 9");
    assert!(
        String::from_utf8_lossy(&quiet.stderr).contains("warning:"),
        "quiet keeps stderr diagnostics"
    );

    // --scan runs the same collection: delivery + arbitration hold there too.
    let scan = run_libra_command(
        &["--exit-code-on-warning", "status", "--scan", "--exit-code"],
        repo.path(),
    );
    assert_eq!(scan.status.code(), Some(9), "scan path: warning exit 9");
    assert!(
        String::from_utf8_lossy(&scan.stderr).contains("warning:"),
        "scan path delivers stderr warnings"
    );
}

/// Exceeding the similarity-comparison budget surfaces
/// `similarity_budget_exceeded` end to end: 750 deleted paths sharing one
/// blob OID × 750 added paths sharing another are only TWO object reads
/// (per-OID caching) but 562k inexact comparisons > the 500k budget.
#[test]
fn similarity_budget_warning() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    for i in 0..750 {
        fs::write(
            repo.path().join(format!("old{i}.txt")),
            "shared alpha payload with enough content to hash\n",
        )
        .unwrap();
    }
    let add = run_libra_command(&["add", "."], repo.path());
    assert_cli_success(&add, "stage base");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit base");
    for i in 0..750 {
        fs::remove_file(repo.path().join(format!("old{i}.txt"))).unwrap();
        fs::write(
            repo.path().join(format!("zz{i}.dat")),
            "shared omega payload with enough content to hash\n",
        )
        .unwrap();
    }
    let add = run_libra_command(&["add", "."], repo.path());
    assert_cli_success(&add, "stage churn");

    let json = run_libra_command(&["--json", "status"], repo.path());
    assert_cli_success(&json, "json status");
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&json.stdout)).expect("envelope");
    assert!(
        doc["data"]["warnings"]
            .as_array()
            .expect("warnings array")
            .iter()
            .any(|w| w["code"] == "similarity_budget_exceeded"),
        "budget exhaustion surfaces end to end: {doc}"
    );
}

// ── R0-4 (first slice): bare -z/--null format arbitration ───────────────────

/// Bare `-z` with no explicit format forces porcelain v1 + NUL records
/// (§B.6): machine intent, not a NUL-terminated human format.
#[test]
fn bare_z_emits_porcelain_v1() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    fs::write(repo.path().join("b.txt"), "untracked\n").unwrap();
    let out = run_libra_command(&["status", "-z"], repo.path());
    assert_cli_success(&out, "bare -z status");
    let raw = String::from_utf8_lossy(&out.stdout);
    assert!(
        raw.contains("?? b.txt\0"),
        "porcelain v1 NUL records: {raw:?}"
    );
    assert!(
        !raw.contains("Untracked files"),
        "no human sections under bare -z: {raw:?}"
    );
}

/// The `st` alias behaves identically for bare `-z`.
#[test]
fn st_bare_z_emits_porcelain_v1() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    fs::write(repo.path().join("b.txt"), "untracked\n").unwrap();
    let out = run_libra_command(&["st", "-z"], repo.path());
    assert_cli_success(&out, "st -z");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("?? b.txt\0"),
        "alias parity"
    );
}

/// `--null` is the Git-parity long alias of `-z`, with the same bare-format
/// forcing; combining it with `--long` or `--cached` fails closed.
#[test]
fn bare_null_emits_porcelain_v1() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    fs::write(repo.path().join("b.txt"), "untracked\n").unwrap();
    let out = run_libra_command(&["status", "--null"], repo.path());
    assert_cli_success(&out, "bare --null status");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("?? b.txt\0"),
        "--null ≡ bare -z"
    );
    let conflict = run_libra_command(&["status", "--long", "--null"], repo.path());
    assert!(!conflict.status.success(), "--long --null fails closed");
    let cached = run_libra_command(&["status", "--cached", "-z"], repo.path());
    assert!(!cached.status.success(), "--cached -z fails closed");
    let scan = run_libra_command(&["status", "--scan", "-z"], repo.path());
    assert!(!scan.status.success(), "--scan -z fails closed");
}

// ── R0-4: Git raw --find-renames grammar + three-way last-wins ──────────────

/// Git raw score grammar reaches status via the argv resolution, verified
/// with an INEXACT rename (~70% similar) so the threshold actually decides:
/// `=505` (50.5%) pairs it, `=80%` splits it, and the three spellings obey
/// last-one-wins including `--renames`.
#[test]
fn find_renames_raw_grammar_and_last_wins() {
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    // ~30% of lines changed → similarity ≈ 70%: between 50.5% and 80%.
    let mut edited = base.clone();
    for i in 0..12 {
        edited = edited.replace(&format!("line {i}\n"), &format!("edited {i}\n"));
    }
    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    fs::write(repo.path().join("moved.txt"), edited).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited move");

    let arrow = "R  orig.txt -> moved.txt";
    let run = |args: &[&str]| {
        let out = run_libra_command(args, repo.path());
        assert_cli_success(&out, "status run");
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    // Raw integer 505 = 50.5%: pairs the ~70% rename.
    assert!(
        run(&["status", "--porcelain=v1", "--find-renames=505"]).contains(arrow),
        "=505 (50.5%) pairs a ~70% rename"
    );
    // 80% literal percent: does NOT pair → threshold is really applied.
    let strict = run(&["status", "--porcelain=v1", "--find-renames=80%"]);
    assert!(
        !strict.contains("->") && strict.contains("orig.txt"),
        "=80% splits the ~70% rename: {strict:?}"
    );
    // Decimal form equivalence.
    assert!(
        run(&["status", "--porcelain=v1", "--find-renames=0.505"]).contains(arrow),
        "decimal raw form works"
    );
    // Three-way last-wins including --renames: strict 80% is overridden by a
    // later --renames (50% default) → pairs again.
    assert!(
        run(&[
            "status",
            "--porcelain=v1",
            "--find-renames=80%",
            "--renames"
        ])
        .contains(arrow),
        "--renames after a strict threshold rewinds to the 50% default"
    );
    // Disable last vs find last.
    let disabled = run(&[
        "status",
        "--porcelain=v1",
        "--find-renames=505",
        "--no-renames",
    ]);
    assert!(!disabled.contains("->"), "--no-renames last disables");
    assert!(
        run(&[
            "status",
            "--porcelain=v1",
            "--no-renames",
            "--find-renames=505"
        ])
        .contains(arrow),
        "--find-renames after --no-renames re-enables at 50.5%"
    );
    // An optional-value GLOBAL before the subcommand must not shift the
    // normalizer's status detection: raw grammar still works under --json.
    let global = run_libra_command(&["--json", "status", "--find-renames=505"], repo.path());
    assert_cli_success(&global, "--json + raw grammar");
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&global.stdout)).expect("envelope");
    assert!(
        doc["data"]["renames"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "raw threshold applied under a global optional-value flag: {doc}"
    );

    // Invalid raw LAST fails closed; invalid overridden by later valid wins.
    let bad_last = run_libra_command(
        &["status", "--find-renames=505", "--find-renames=foo"],
        repo.path(),
    );
    assert!(!bad_last.status.success(), "invalid last raw fails closed");
    assert!(
        run(&[
            "status",
            "--porcelain=v1",
            "--find-renames=foo",
            "--find-renames=505"
        ])
        .contains(arrow),
        "invalid overridden by later valid wins"
    );
}

/// `status`/`st` as another command's argument is never rewritten. The pin:
/// diff's own bare `--find-renames` EATS the next token as its value (clap
/// `num_args=0..=1`), so `libra diff --find-renames status` must fail with
/// diff's invalid-value error — if the normalizer had rewritten the bare
/// flag to a placeholder, `status` would have survived as a pathspec and the
/// command would behave differently.
#[test]
fn normalize_ignores_diff_pathspec_status() {
    let repo = create_repo_with_committed_file("a.txt", "hello rename world\ncontent line two\n");
    let eaten = run_libra_command(&["diff", "--find-renames", "status"], repo.path());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&eaten.stdout),
        String::from_utf8_lossy(&eaten.stderr)
    );
    assert!(
        !eaten.status.success() && combined.contains("status"),
        "diff still eats 'status' as its --find-renames value (no rewrite): {combined}"
    );
    // Same for the `st` spelling.
    let st = run_libra_command(&["diff", "--find-renames", "st"], repo.path());
    assert!(
        !st.status.success(),
        "'st' after diff's bare --find-renames stays its (invalid) value"
    );
    // And in status itself the SAME shape succeeds because the placeholder
    // rewrite protects the pathspec.
    let ours = run_libra_command(&["status", "--find-renames", "a.txt"], repo.path());
    assert_cli_success(&ours, "status bare --find-renames keeps the pathspec");
}

// ── R0-2 residuals: designated snapshot/evidence tests ───────────────────────

/// A repository with no HEAD commit (unborn branch) must not crash rename
/// detection: everything staged is a plain `new file`, the JSON `renames[]`
/// array is empty, and the run succeeds.
#[test]
fn no_head_staged_rename() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::write(repo.path().join("fresh.txt"), "no head yet\n").unwrap();
    let add = run_libra_command(&["add", "fresh.txt"], repo.path());
    assert_cli_success(&add, "stage file on unborn HEAD");

    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("new file") && out.contains("fresh.txt") && !out.contains("renamed:"),
        "unborn-HEAD staging must render as a plain add: {out}"
    );

    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    assert_eq!(
        doc["data"]["renames"].as_array().map(Vec::len),
        Some(0),
        "no rename entries can exist without a HEAD side: {json}"
    );
}

/// §B.5 delivery matrix: JSON paths are repository-root-relative even when
/// `status` runs from a subdirectory, and the score/exactness lookup still
/// resolves (no silent 100/exact fallback).
#[test]
fn json_repo_relative_from_subdir() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    fs::create_dir(repo.path().join("sub")).unwrap();
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fs::write(repo.path().join("sub/a.txt"), &base).unwrap();
    let add = run_libra_command(&["add", "sub/a.txt"], repo.path());
    assert_cli_success(&add, "stage subdir file");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit subdir file");
    let mv = run_libra_command(&["mv", "sub/a.txt", "sub/b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv in subdir");
    // Make the rename inexact so a details-map miss (which would fall back
    // to score=100/exact=true) is detectable.
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("sub/b.txt"), edited).unwrap();
    let add = run_libra_command(&["add", "sub/b.txt"], repo.path());
    assert_cli_success(&add, "restage edited moved file");

    let out = status_stdout(&repo.path().join("sub"), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json status");
    let renames = doc["data"]["renames"]
        .as_array()
        .expect("renames array present");
    let entry = renames
        .iter()
        .find(|r| r["to"] == "sub/b.txt")
        .unwrap_or_else(|| panic!("rename entry must be repo-relative: {out}"));
    assert_eq!(entry["from"], "sub/a.txt", "from is repo-relative: {out}");
    assert_eq!(
        entry["exact"], false,
        "edited rename must be inexact: {out}"
    );
    let score = entry["score"].as_u64().expect("score");
    assert!(
        (50..100).contains(&score),
        "inexact score must come from the real details map, got {score}: {out}"
    );
    // The staged change set is repo-relative too.
    let staged = doc["data"]["staged"]["renamed"]
        .as_array()
        .expect("staged renamed");
    assert!(
        staged
            .iter()
            .any(|p| p["from"] == "sub/a.txt" && p["to"] == "sub/b.txt"),
        "staged.renamed must be repo-relative: {out}"
    );
}

/// §B.4.1 empty-file rule end-to-end: an empty file pairs EXACT (same OID)
/// on a pure move, but never joins inexact scoring — an empty source plus a
/// non-empty replacement stays a delete + add.
#[test]
fn rename_empty_file_exact_pair_only() {
    // Pure move of an empty file: exact rename.
    let repo = create_repo_with_committed_file("empty.txt", "");
    let mv = run_libra_command(&["mv", "empty.txt", "renamed-empty.txt"], repo.path());
    assert_cli_success(&mv, "mv empty file");
    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("renamed-empty.txt"),
        "empty-file pure move must pair exactly by OID: {out}"
    );

    // Empty source + non-empty destination: no inexact pairing.
    let repo = create_repo_with_committed_file("empty.txt", "");
    let rm = run_libra_command(&["rm", "empty.txt"], repo.path());
    assert_cli_success(&rm, "delete empty file");
    fs::write(repo.path().join("full.txt"), "now with content\n").unwrap();
    let add = run_libra_command(&["add", "full.txt"], repo.path());
    assert_cli_success(&add, "stage replacement");
    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        !out.contains("renamed:"),
        "an empty file must never pair inexactly: {out}"
    );
    assert!(
        out.contains("deleted:") && out.contains("new file"),
        "the endpoints stay a delete + add: {out}"
    );
}

/// GC-02 hash-kind neutrality: rename detection (exact and inexact) works
/// identically in a SHA-256 repository.
#[test]
fn rename_sha256_repo_detected() {
    let repo = tempdir().expect("temp repo");
    fs::create_dir_all(repo.path()).unwrap();
    let init = run_libra_command(&["init", "--object-format", "sha256"], repo.path());
    assert_cli_success(&init, "init sha256 repo");
    configure_identity_via_cli(repo.path());
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fs::write(repo.path().join("wide.txt"), &base).unwrap();
    let add = run_libra_command(&["add", "wide.txt"], repo.path());
    assert_cli_success(&add, "stage sha256 fixture");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit sha256 fixture");

    // Exact rename.
    let mv = run_libra_command(&["mv", "wide.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "mv in sha256 repo");
    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("moved.txt"),
        "sha256 exact rename must be detected: {out}"
    );

    // Inexact rename after a small edit.
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("moved.txt"), edited).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited sha256 file");
    let out = status_stdout(repo.path(), &["status"]);
    assert!(
        out.contains("renamed:") && out.contains("moved.txt"),
        "sha256 inexact rename must be detected: {out}"
    );
}

// ── R0-5 residuals: porcelain v2 rename record field pins (§B.6.4) ───────────

/// Parse the first `2 …` record of a porcelain v2 dump into whitespace
/// fields (paths carry no spaces in these fixtures, so `new`/`old` land at
/// indices 9/10).
fn first_v2_record(out: &str) -> Vec<String> {
    let line = out
        .lines()
        .find(|l| l.starts_with("2 "))
        .unwrap_or_else(|| panic!("expected a porcelain v2 rename record: {out}"));
    line.split_whitespace().map(str::to_string).collect()
}

/// Staged inexact rename with a mode flip: every metadata field of the
/// `2 R.` record is pinned against fixture-known modes and OIDs
/// (§B.6.4 field order `2 <xy> <sub> <mH> <mI> <mW> <hH> <hI> R<pct>
/// <new>\t<old>`).
#[test]
fn porcelain_v2_staged_rename_mode_hash_fields() {
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    let head_oid = {
        let out = run_libra_command(&["rev-parse", "HEAD:orig.txt"], repo.path());
        assert_cli_success(&out, "rev-parse HEAD:orig.txt");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("moved.txt"), &edited).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            repo.path().join("moved.txt"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited moved file");
    // Fixture truth for the index side from ls-files --stage.
    let stage = {
        let out = run_libra_command(&["ls-files", "--stage"], repo.path());
        assert_cli_success(&out, "ls-files --stage");
        String::from_utf8(out.stdout).unwrap()
    };
    let stage_line = stage
        .lines()
        .find(|l| l.ends_with("moved.txt"))
        .unwrap_or_else(|| panic!("staged entry for moved.txt: {stage}"));
    let stage_fields: Vec<&str> = stage_line.split_whitespace().collect();
    let (index_mode, index_oid) = (stage_fields[0], stage_fields[1]);

    let out = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let fields = first_v2_record(&out);
    assert_eq!(fields[1], "R.", "staged-only rename xy: {out}");
    assert_eq!(fields[2], "N...", "ordinary entry sub field: {out}");
    assert_eq!(fields[3], "100644", "mH is the committed mode: {out}");
    assert_eq!(fields[4], index_mode, "mI matches ls-files --stage: {out}");
    assert_eq!(fields[5], index_mode, "mW matches the on-disk mode: {out}");
    assert_eq!(fields[6], head_oid, "hH is the HEAD blob: {out}");
    assert_eq!(fields[7], index_oid, "hI matches ls-files --stage: {out}");
    assert_ne!(fields[6], fields[7], "content edit keeps hH != hI: {out}");
    let pct: u32 = fields[8]
        .strip_prefix('R')
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("score field R<pct>: {out}"));
    assert!((50..100).contains(&pct), "inexact score in range: {out}");
    assert_eq!(fields[9], "moved.txt", "new path first: {out}");
    assert_eq!(fields[10], "orig.txt", "old path second: {out}");
}

/// Unstaged-only rename (`.R`, `status.renameUntracked=true`): Git copies the
/// index fields into the HEAD columns — hH == hI == the REAL index OID and
/// mH == mI == the real mode; the all-zero fallback must fail this test.
#[test]
fn porcelain_v2_unstaged_dot_r_hash_fixup() {
    let repo = create_repo_with_committed_file("a.txt", "hash fixup content\nsecond line\n");
    let cfg = run_libra_command(&["config", "status.renameUntracked", "true"], repo.path());
    assert_cli_success(&cfg, "enable renameUntracked");
    let index_oid = {
        let out = run_libra_command(&["rev-parse", "HEAD:a.txt"], repo.path());
        assert_cli_success(&out, "rev-parse HEAD:a.txt");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    // Pure worktree move: index keeps a.txt, disk has b.txt.
    fs::rename(repo.path().join("a.txt"), repo.path().join("b.txt")).unwrap();

    let out = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let fields = first_v2_record(&out);
    assert_eq!(fields[1], ".R", "unstaged-only rename xy: {out}");
    assert_eq!(fields[3], "100644", "mH copies the index mode: {out}");
    assert_eq!(fields[4], "100644", "mI is the index mode: {out}");
    assert_eq!(fields[5], "100644", "mW is the real worktree mode: {out}");
    assert_eq!(fields[6], index_oid, "hH copies the index OID: {out}");
    assert_eq!(fields[7], index_oid, "hI is the index OID: {out}");
    assert_eq!(fields[8], "R100", "pure move is exact: {out}");
    assert_eq!(fields[9], "b.txt", "new path first: {out}");
    assert_eq!(fields[10], "a.txt", "old path second: {out}");
}

/// A staged rename chained with a worktree rename produces exactly two v2
/// records (`R.` old→mid, `.R` mid→new), two short-format lines, and two
/// JSON entries — never a merged or dropped hop.
#[test]
fn chain_rename_two_records() {
    let repo = create_repo_with_committed_file("a.txt", "chain rename content\nsecond line\n");
    let cfg = run_libra_command(&["config", "status.renameUntracked", "true"], repo.path());
    assert_cli_success(&cfg, "enable renameUntracked");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "staged hop a->b");
    fs::rename(repo.path().join("b.txt"), repo.path().join("c.txt")).unwrap();

    let v2 = status_stdout(repo.path(), &["status", "--porcelain=v2"]);
    let records: Vec<&str> = v2.lines().filter(|l| l.starts_with("2 ")).collect();
    assert_eq!(records.len(), 2, "exactly two rename records: {v2}");
    assert!(
        records
            .iter()
            .any(|r| r.contains(" R. ") && r.ends_with("b.txt\ta.txt")),
        "staged hop a->b: {v2}"
    );
    assert!(
        records
            .iter()
            .any(|r| r.contains(" .R ") && r.ends_with("c.txt\tb.txt")),
        "worktree hop b->c: {v2}"
    );

    let short = status_stdout(repo.path(), &["status", "--short"]);
    let arrows = short.lines().filter(|l| l.contains(" -> ")).count();
    assert_eq!(arrows, 2, "two short-format rename lines: {short}");

    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    let renames = doc["data"]["renames"].as_array().expect("renames array");
    assert_eq!(renames.len(), 2, "two JSON rename entries: {json}");
    assert!(
        renames
            .iter()
            .any(|r| r["from"] == "a.txt" && r["to"] == "b.txt" && r["staged"] == true)
    );
    assert!(
        renames
            .iter()
            .any(|r| r["from"] == "b.txt" && r["to"] == "c.txt" && r["unstaged"] == true)
    );
}

/// Porcelain v2 under `-z`: the rename record ends `… R<pct> <new> NUL <old>
/// NUL` with no trailing newline after the final NUL (§B.6.4).
#[test]
fn porcelain_v2_z_rename_record_nul_paths() {
    let repo = create_repo_with_committed_file("a.txt", "nul separated rename\nsecond line\n");
    let mv = run_libra_command(&["mv", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");

    let out = status_stdout(repo.path(), &["status", "--porcelain=v2", "-z"]);
    assert!(
        out.contains("R100 b.txt\0a.txt\0"),
        "-z rename paths are NUL separated, new first: {out:?}"
    );
    assert!(
        !out.contains("b.txt\ta.txt"),
        "-z must not fall back to the TAB form: {out:?}"
    );
    assert!(
        out.ends_with('\0') && !out.ends_with("\0\n"),
        "records are NUL terminated with no trailing newline: {out:?}"
    );
}

// ── R0-7 residuals: renameLimit cascade + JSON score contracts ───────────────

/// Build a repo with two committed files whose basenames do NOT recur on the
/// destination side, forcing rename pairing through the bounded exhaustive
/// stage (the per-side renameLimit's only gate).
fn repo_with_two_exhaustive_candidates() -> tempfile::TempDir {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    let base_a: String = (0..40).map(|i| format!("alpha {i}\n")).collect();
    let base_b: String = (0..40).map(|i| format!("beta {i}\n")).collect();
    fs::write(repo.path().join("a1.txt"), &base_a).unwrap();
    fs::write(repo.path().join("a2.txt"), &base_b).unwrap();
    let add = run_libra_command(&["add", "a1.txt", "a2.txt"], repo.path());
    assert_cli_success(&add, "stage exhaustive fixtures");
    let commit = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit exhaustive fixtures");
    // Delete both and stage differently named, lightly edited replacements.
    let rm = run_libra_command(&["rm", "a1.txt", "a2.txt"], repo.path());
    assert_cli_success(&rm, "delete old side");
    fs::write(
        repo.path().join("b1.txt"),
        base_a.replace("alpha 5\n", "alpha five\n"),
    )
    .unwrap();
    fs::write(
        repo.path().join("b2.txt"),
        base_b.replace("beta 5\n", "beta five\n"),
    )
    .unwrap();
    let add = run_libra_command(&["add", "b1.txt", "b2.txt"], repo.path());
    assert_cli_success(&add, "stage new side");
    repo
}

/// `status.renameLimit` cascades (falling back to `diff.renameLimit`, CLI
/// semantics: 0 = uncapped) and gates ONLY the exhaustive stage (§B.5).
#[test]
fn rename_limit_config_cascade() {
    // status.renameLimit=1 skips the 2×2 exhaustive stage with the warning.
    let repo = repo_with_two_exhaustive_candidates();
    let cfg = run_libra_command(&["config", "status.renameLimit", "1"], repo.path());
    assert_cli_success(&cfg, "set status.renameLimit");
    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    assert_eq!(doc["data"]["renames"].as_array().map(Vec::len), Some(0));
    assert!(
        json.contains("rename_limit_product_skipped"),
        "limit warning surfaces: {json}"
    );

    // diff.renameLimit is the fallback when the status key is absent...
    let repo = repo_with_two_exhaustive_candidates();
    let cfg = run_libra_command(&["config", "diff.renameLimit", "1"], repo.path());
    assert_cli_success(&cfg, "set diff.renameLimit");
    let json = status_stdout(repo.path(), &["--json", "status"]);
    assert!(
        json.contains("rename_limit_product_skipped"),
        "diff.renameLimit fallback applies: {json}"
    );

    // ...and status.renameLimit wins over it; 0 disables the cap entirely.
    let cfg = run_libra_command(&["config", "status.renameLimit", "0"], repo.path());
    assert_cli_success(&cfg, "override with status.renameLimit=0");
    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    assert_eq!(
        doc["data"]["renames"].as_array().map(Vec::len),
        Some(2),
        "status.renameLimit=0 uncaps and wins over diff.renameLimit: {json}"
    );
    assert!(!json.contains("rename_limit_product_skipped"), "{json}");

    // Invalid values fail closed before any output.
    let cfg = run_libra_command(&["config", "status.renameLimit", "banana"], repo.path());
    assert_cli_success(&cfg, "set invalid status.renameLimit");
    let out = run_libra_command(&["status"], repo.path());
    assert!(
        !out.status.success(),
        "invalid renameLimit must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("status.renameLimit"),
        "failure names the key: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// JSON chain contract: a staged inexact rename whose destination is also
/// modified in the worktree (`RM` in short form) keeps ONE renames[] entry
/// with the real partial score plus the unstaged modification listed
/// separately.
#[test]
fn json_rename_rm_partial_chain() {
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("moved.txt"), &edited).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited moved file");
    // Worktree-only edit on top of the staged rename destination.
    fs::write(
        repo.path().join("moved.txt"),
        edited.replace("line 9\n", "line nine\n"),
    )
    .unwrap();

    let short = status_stdout(repo.path(), &["status", "--short"]);
    assert!(
        short.lines().any(|l| l.starts_with("RM ")),
        "short form renders RM: {short}"
    );

    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    let renames = doc["data"]["renames"].as_array().expect("renames array");
    assert_eq!(renames.len(), 1, "one rename entry: {json}");
    let entry = &renames[0];
    assert_eq!(entry["from"], "orig.txt");
    assert_eq!(entry["to"], "moved.txt");
    assert_eq!(entry["exact"], false);
    let score = entry["score"].as_u64().expect("score");
    assert!((50..100).contains(&score), "partial score: {json}");
    assert!(
        doc["data"]["unstaged"]["modified"]
            .as_array()
            .expect("unstaged modified")
            .iter()
            .any(|p| p == "moved.txt"),
        "worktree edit listed as unstaged modification: {json}"
    );
}

/// Spanhash similarity is line-multiset based: a fully reordered file scores
/// internal 60000 → JSON `score: 100` while staying `exact: false` (the
/// OIDs differ).
#[test]
fn json_inexact_reordered_score_100() {
    let lines: Vec<String> = (0..40).map(|i| format!("line {i}\n")).collect();
    let base: String = lines.concat();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    let reversed: String = lines.iter().rev().cloned().collect::<Vec<_>>().concat();
    fs::write(repo.path().join("moved.txt"), reversed).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage reordered file");

    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    let renames = doc["data"]["renames"].as_array().expect("renames array");
    assert_eq!(renames.len(), 1, "{json}");
    assert_eq!(renames[0]["exact"], false, "OIDs differ: {json}");
    assert_eq!(
        renames[0]["score"], 100,
        "reordered lines score 100: {json}"
    );
}

/// Partial similarity floors (Git floor semantics, §B.9 59999→99): an edited
/// rename reports the floored percentage, never a rounded-up 100.
#[test]
fn json_inexact_spanhash_score_floor() {
    let base: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let repo = create_repo_with_committed_file("orig.txt", &base);
    let mv = run_libra_command(&["mv", "orig.txt", "moved.txt"], repo.path());
    assert_cli_success(&mv, "libra mv");
    let edited = base.replace("line 5\n", "line five changed\n");
    fs::write(repo.path().join("moved.txt"), edited).unwrap();
    let add = run_libra_command(&["add", "moved.txt"], repo.path());
    assert_cli_success(&add, "restage edited file");

    let json = status_stdout(repo.path(), &["--json", "status"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json status");
    let renames = doc["data"]["renames"].as_array().expect("renames array");
    assert_eq!(renames.len(), 1, "{json}");
    assert_eq!(renames[0]["exact"], false, "{json}");
    let score = renames[0]["score"].as_u64().expect("score");
    assert!(
        (50..100).contains(&score),
        "one edited line floors below 100: {json}"
    );
}
