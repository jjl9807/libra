//! Integration coverage for `libra graph` CLI argument handling.

use std::{fs, path::Path};

use sea_orm::{ConnectionTrait, Database};
use tempfile::tempdir;

use super::{assert_cli_success, parse_cli_error_stderr, parse_json_stdout, run_libra_command};

#[test]
fn graph_rejects_non_uuid_thread_id() {
    let repo = tempdir().expect("failed to create temporary directory");
    let init = run_libra_command(&["init"], repo.path());
    assert_cli_success(&init, "failed to initialize repository");

    // `--json` selects the structured path so the run reaches UUID validation
    // instead of the W5-08 interactive-entry refusal.
    let output = run_libra_command(&["graph", "--json", "not-a-thread"], repo.path());

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("graph expects a canonical thread_id UUID"),
        "expected graph UUID validation error, got {:?}",
        report
    );
}

#[test]
fn graph_repo_flag_uses_target_repo_when_passed_after_thread_id() {
    let root = tempdir().expect("failed to create temporary directory");
    let repo = root.path().join("linked");
    let outside = root.path().join("outside");
    fs::create_dir_all(&repo).expect("failed to create repository directory");
    fs::create_dir_all(&outside).expect("failed to create outside directory");

    let init = run_libra_command(&["init"], &repo);
    assert_cli_success(&init, "failed to initialize repository");

    let repo_arg = repo
        .to_str()
        .expect("temporary repository path should be valid UTF-8");
    // `--json` selects the structured path so the run reaches the graph load
    // instead of the interactive-entry refusal; the load failure then proves
    // graph resolved the `--repo` target.
    let output = run_libra_command(
        &[
            "graph",
            "--json",
            "019d9c35-5e95-7901-9625-65abdf797165",
            "--repo",
            repo_arg,
        ],
        &outside,
    );

    assert!(!output.status.success());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        report.message.contains("failed to load thread graph"),
        "expected graph load failure after accepting --repo, got {:?}",
        report
    );
    assert!(
        !report.message.contains("unexpected argument"),
        "graph should accept --repo after the thread id, got {:?}",
        report
    );
    assert!(
        !report.message.contains("not a libra repository"),
        "graph should use the --repo target instead of the process cwd, got {:?}",
        report
    );
}

/// `libra graph --help` surfaces the EXAMPLES banner so users see the
/// `--json` / `--machine` structured-output forms and the `--repo` override
/// without reading the design doc. Cross-cutting `--help` EXAMPLES rollout
/// per `docs/development/commands/_general.md` item B. The interactive TUI
/// invocation was removed in the W5 breaking release and must not reappear.
#[test]
fn test_graph_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for graph --help");
    let output = run_libra_command(&["graph", "--help"], repo.path());
    assert!(
        output.status.success(),
        "graph --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "graph --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra graph --json <thread-uuid>",
        "libra graph --machine <thread-uuid>",
        "--repo /path/to/repo",
    ] {
        assert!(
            stdout.contains(invocation),
            "graph --help should include `{invocation}`, stdout: {stdout}"
        );
    }
    assert!(
        !stdout.contains("Deprecated TUI"),
        "graph --help must not advertise the removed interactive TUI entry, stdout: {stdout}"
    );
}

/// Seed a minimal thread projection so `graph --json` has a loadable graph:
/// one `ai_thread` row, its scheduler state row (keeps the projection on the
/// `Fresh` path, skipping the history rebuild), and one head intent. Missing
/// AI-history objects degrade to `unavailable` object details, so no object
/// blobs are needed for the wire-compat assertions.
fn seed_thread_projection(repo: &Path, thread_id: &str, intent_id: &str) {
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime.block_on(async {
        let url = format!(
            "sqlite://{}?mode=rwc",
            repo.join(".libra/libra.db").display()
        );
        let connection = Database::connect(&url)
            .await
            .expect("open repo db for seeding");
        connection
            .execute_unprepared(&format!(
                "INSERT INTO ai_thread (thread_id, title, owner_kind, owner_id, \
                 current_intent_id, latest_intent_id, archived, version, created_at, updated_at) \
                 VALUES ('{thread_id}', 'Graph machine test', 'human', 'graph-test', \
                 '{intent_id}', '{intent_id}', 0, 1, 1, 2);
                 INSERT INTO ai_thread_intent (thread_id, intent_id, ordinal, is_head, \
                 linked_at, link_reason) \
                 VALUES ('{thread_id}', '{intent_id}', 0, 1, 1, 'seed');
                 INSERT INTO ai_scheduler_state (thread_id, version, updated_at) \
                 VALUES ('{thread_id}', 1, 2);"
            ))
            .await
            .expect("seed thread projection");
    });
}

/// Breaking removal: the interactive graph TUI entry is gone. A bare
/// `libra graph <thread-uuid>` is refused with a stable usage error and a
/// migration hint pointing at Web Code UI and the structured flags, while
/// `--json` / `--machine` keep the structured wire byte-compatible.
#[test]
fn graph_machine_survives_tui_removal() {
    let repo = tempdir().expect("failed to create temporary directory");
    let init = run_libra_command(&["init"], repo.path());
    assert_cli_success(&init, "failed to initialize repository");

    let thread_id = "11111111-1111-4111-8111-111111111111";
    let intent_id = "22222222-2222-4222-8222-222222222222";
    seed_thread_projection(repo.path(), thread_id, intent_id);

    // 1. The bare interactive entry is refused with the migration hint.
    let output = run_libra_command(&["graph", thread_id], repo.path());
    assert!(
        !output.status.success(),
        "bare `libra graph` must be refused after the interactive TUI removal"
    );
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("no longer opens an interactive TUI"),
        "expected the breaking refusal message, got {:?}",
        report
    );
    let hints = report.hints.join("\n");
    for needle in ["Web Code UI", "--json", "--machine"] {
        assert!(
            hints.contains(needle),
            "migration hint must mention `{needle}`, hints: {hints}"
        );
    }

    // 2. `--json` keeps the wire: envelope plus thread metadata and nodes.
    let output = run_libra_command(&["graph", "--json", thread_id], repo.path());
    assert_cli_success(&output, "graph --json should keep working");
    let payload = parse_json_stdout(&output);
    assert_eq!(payload["command"], "graph");
    assert_eq!(payload["data"]["thread_id"], thread_id);
    assert_eq!(payload["data"]["title"], "Graph machine test");
    let nodes = payload["data"]["nodes"]
        .as_array()
        .expect("nodes should be an array");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["kind"], "intent");
    assert_eq!(nodes[0]["id"], intent_id);
    assert!(
        nodes[0]["tags"]
            .as_array()
            .expect("tags should be an array")
            .iter()
            .any(|tag| tag == "head"),
        "seeded head intent must keep its `head` tag, nodes: {nodes:?}"
    );

    // 3. `--machine` emits the same envelope as compact single-line JSON.
    let output = run_libra_command(&["graph", "--machine", thread_id], repo.path());
    assert_cli_success(&output, "graph --machine should keep working");
    let payload = parse_json_stdout(&output);
    assert_eq!(payload["command"], "graph");
    assert_eq!(payload["data"]["thread_id"], thread_id);
    assert!(
        payload["data"]["nodes"].is_array(),
        "machine output must keep the nodes array"
    );
}
