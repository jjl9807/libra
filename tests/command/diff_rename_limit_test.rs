//! Large-set regression for default rename detection.
//!
//! plan-20260714 §B.4.2.3 stage order: exact → unique-basename (ALWAYS
//! runs) → bounded exhaustive. A tripped per-side `renameLimit`
//! therefore skips only the exhaustive stage; the inexact fixture below
//! deliberately uses DISTINCT basenames (`src-NNNN` → `dst-NNNN`) so the
//! basename stage cannot pair it and the limit is genuinely exercised.
//!
//! Layer: L1 (deterministic; tempdir only, no network).

use std::{fs, time::Instant};

use super::{
    assert_cli_success, create_committed_repo_via_cli, run_libra_command,
    run_libra_command_with_stdin_and_env,
};

const LARGE_SET: usize = 1001;

#[test]
fn diff_large_set_warns_and_preserves_exact_renames() {
    let repo = create_committed_repo_via_cli();
    let root = repo.path();
    let exact_old = root.join("exact-old");
    let exact_new = root.join("exact-new");
    let inexact_old = root.join("inexact-old");
    let inexact_new = root.join("inexact-new");
    for dir in [&exact_old, &exact_new, &inexact_old, &inexact_new] {
        fs::create_dir_all(dir).expect("create large-set fixture directory");
    }

    for index in 0..LARGE_SET {
        fs::write(
            exact_old.join(format!("{index:04}.txt")),
            format!("exact-{index}\n"),
        )
        .expect("write exact source");
        fs::write(
            inexact_old.join(format!("src-{index:04}.txt")),
            format!("old-{index}\n"),
        )
        .expect("write inexact source");
    }
    assert_cli_success(&run_libra_command(&["add", "-A"], root), "stage base set");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "large base", "--no-verify"], root),
        "commit base set",
    );

    fs::remove_dir_all(&exact_old).expect("remove exact source directory");
    fs::remove_dir_all(&inexact_old).expect("remove inexact source directory");
    for index in 0..LARGE_SET {
        fs::write(
            exact_new.join(format!("{index:04}.txt")),
            format!("exact-{index}\n"),
        )
        .expect("write exact destination");
        fs::write(
            inexact_new.join(format!("dst-{index:04}.txt")),
            format!("new-{index}\n"),
        )
        .expect("write inexact destination");
    }
    assert_cli_success(
        &run_libra_command(&["add", "-A"], root),
        "stage large rename set",
    );

    let output = run_libra_command(&["diff", "--staged", "--summary"], root);
    assert_cli_success(&output, "diff large rename set");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("skipped inexact rename detection"),
        "missing rename-limit warning: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches(" rename ").count(),
        LARGE_SET,
        "all exact renames must survive the limit"
    );
    assert!(stdout.contains("exact-old") && stdout.contains("exact-new"));
}

/// WIO-03: `diff -M` inexact object reads use the killable ObjectReadBudget
/// path (detection + rendering). A hung store read must complete promptly
/// without inventing a rename or leaving helper children.
#[test]
#[cfg(unix)]
fn diff_rename_object_read_budget_timeout_is_cancellable() {
    let old_body = "diff cancellable line one\nline two\nline three\nline four\n";
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("old.txt"), old_body).unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "old.txt"], repo.path()),
        "stage old.txt",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path()),
        "commit old.txt",
    );
    let ls = run_libra_command(&["ls-tree", "HEAD"], repo.path());
    assert_cli_success(&ls, "ls-tree HEAD");
    let listing = String::from_utf8_lossy(&ls.stdout).into_owned();
    let hash = listing
        .lines()
        .find(|l| l.ends_with("old.txt"))
        .and_then(|l| l.split_whitespace().nth(2))
        .expect("old.txt blob hash")
        .to_string();

    fs::remove_file(repo.path().join("old.txt")).unwrap();
    fs::write(
        repo.path().join("new.txt"),
        "diff cancellable line one\nline two CHANGED\nline three\nline four\n",
    )
    .unwrap();
    let add = run_libra_command(&["add", "."], repo.path());
    assert_cli_success(&add, "stage inexact move");

    let before = Instant::now();
    let out = run_libra_command_with_stdin_and_env(
        &["diff", "--staged", "-M", "--summary"],
        repo.path(),
        "",
        &[
            ("LIBRA_TEST_SLOW_OBJECT_READ_MS", "8000"),
            ("LIBRA_TEST_SLOW_OBJECT_READ_OID", &hash),
        ],
    );
    let elapsed = before.elapsed();
    assert_cli_success(&out, "diff -M returns after cancelling hung object read");
    assert!(
        elapsed < std::time::Duration::from_secs(12),
        "hung object read must be killed within the budget window: {elapsed:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(" rename "),
        "a cancelled object read must not invent a rename: {stdout}"
    );
    assert!(
        stdout.contains("delete mode") || stdout.contains("create mode") || !stdout.is_empty(),
        "base add/delete summary should still render: {stdout}"
    );
}

/// W5-09 regression (Codex r8): a blob larger than the status I/O worker's
/// 8 MiB binary-frame cap cannot cross the worker boundary, and the worker
/// only sees local `.libra/objects` in the first place — diff detection must
/// fall back to the legacy in-process tiered read for such worker-unservable
/// objects instead of silently unpairing the rename (plan-20260714 §B.7:
/// diff has no content budget; pre-WIO parity). The killed/hung-read case
/// stays bounded — see `diff_rename_object_read_budget_timeout_is_cancellable`.
#[test]
fn diff_rename_pairs_blob_larger_than_worker_frame_cap() {
    let repo = create_committed_repo_via_cli();
    let root = repo.path();
    // ~9 MiB in ~9k KILOBYTE-long lines: past the 8 MiB frame cap, but
    // under the 10,000-line `<LargeFile>` placeholder gate — that gate
    // reclassifies both sides as "modified" before rename detection runs
    // (a separate, pre-existing behavior this test must not entangle).
    let long_line = format!("{}\n", "x".repeat(1023));
    let mut body = String::with_capacity(9 * 1024 * 1024 + 1100);
    body.push_str("header line ORIGINAL\n");
    while body.len() < 9 * 1024 * 1024 {
        body.push_str(&long_line);
    }
    fs::write(root.join("big-old.bin"), &body).expect("write big source");
    assert_cli_success(&run_libra_command(&["add", "-A"], root), "stage big source");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "big base", "--no-verify"], root),
        "commit big source",
    );

    fs::remove_file(root.join("big-old.bin")).expect("remove big source");
    let changed = body.replacen("ORIGINAL", "CHANGED", 1);
    fs::write(root.join("big-new.bin"), &changed).expect("write big destination");
    assert_cli_success(&run_libra_command(&["add", "-A"], root), "stage big rename");

    let output = run_libra_command(&["diff", "--staged", "-M", "--summary"], root);
    assert_cli_success(&output, "diff big rename");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(" rename ") && stdout.contains("big-new.bin"),
        "a blob past the 8 MiB worker frame cap must still pair as a rename: {stdout}"
    );
}
