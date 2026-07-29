//! `libra worktree doctor` — the Part C W4 read-only scope-diagnosis machine
//! interface (plan-20260714 §C.8 / §C.13).
//!
//! Four contracts are pinned here:
//!
//! 1. The default invocation is STRICTLY READ-ONLY — rows, registry, lease and
//!    filesystem are byte-identical before and after.
//! 2. The frozen response shapes: a paginated `data.diagnostics[]` +
//!    `data.next_cursor` view, and a singular `data.diagnostic` view with NO
//!    pagination keys, both under `command = "worktree.doctor"`.
//! 3. Keyset pagination with an OPAQUE cursor (default limit 50, cap 500) and
//!    a fail-closed `LBR-WORKTREE-001` on a cursor this command did not issue.
//! 4. Fail-closed diagnosis: an unreadable scope is `LBR-WORKTREE-002`, never
//!    a silently truncated report; and mixing the single-scope id with
//!    `--limit`/`--cursor` is a usage error (`LBR-CLI-002`).

use std::path::Path;

use libra::internal::workspace::{AcquireRequest, WorkspaceStore, now_ms};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, Statement};

use super::*;

async fn repo_db(repo: &Path) -> sea_orm::DatabaseConnection {
    let url = format!("sqlite://{}", repo.join(".libra/libra.db").display());
    let mut opts = ConnectOptions::new(url);
    opts.sqlx_logging(false);
    Database::connect(opts).await.expect("open repo db")
}

/// This repository's canonical identity, so seeded rows are attributed to it
/// (a different value would be reported as a foreign-identity record).
async fn repo_identity(conn: &sea_orm::DatabaseConnection) -> String {
    let row = conn
        .query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT value FROM config_kv WHERE key = 'libra.repoid' ORDER BY id DESC LIMIT 1",
        ))
        .await
        .expect("query repo identity")
        .expect("repository identity row");
    row.try_get_by_index::<String>(0).expect("identity value")
}

/// Acquire one linked workspace whose directory exists on disk.
/// `acquired_at_ms` drives the lease deadline, so a caller can seed either a
/// live lease or one that expired long ago without sleeping.
async fn seed_linked_workspace(repo: &Path, name: &str, acquired_at_ms: i64) -> String {
    let conn = repo_db(repo).await;
    let dir = repo.join(name);
    fs::create_dir_all(&dir).expect("create workspace dir");
    let request = AcquireRequest::linked(
        format!("wt-{name}"),
        dir.canonicalize().expect("canonical workspace dir"),
        format!("agent-{name}"),
        600_000,
    );
    let lease = WorkspaceStore::acquire(&conn, &request, acquired_at_ms)
        .await
        .expect("acquire linked workspace");
    lease.workspace_id
}

/// Insert `count` plain rows straight through SQL — the only practical way to
/// prove the 500-row page cap without minutes of real acquisitions.
async fn bulk_seed(conn: &sea_orm::DatabaseConnection, repo_id: &str, count: usize) {
    for index in 0..count {
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO workspace_record (workspace_id, repo_id, kind, worktree_id, path, \
             owner_kind, state, lease_fence, created_at, updated_at) \
             VALUES (?, ?, 'remote', NULL, ?, 'agent', 'active', 0, 1, 1)",
            [
                format!("bulk-{index:04}").into(),
                repo_id.into(),
                format!("/nonexistent/bulk-{index:04}").into(),
            ],
        ))
        .await
        .expect("bulk insert workspace row");
    }
}

/// Everything the read-only contract covers: every `workspace_record` row, the
/// worktree registry bytes, and the repository's file tree.
///
/// Two families are filtered out of the tree listing because ANY command that
/// merely opens the repository creates them — `libra worktree list`, a plain
/// reader, creates both — so including them would make the assertion about
/// process startup rather than about this command writing repository state:
///
/// * SQLite's transient sidecars (`-wal`, `-shm`, `-journal`);
/// * the object-index repair advisory lock directory (`.lock` files), which
///   holds no repository state at all.
async fn repo_snapshot(repo: &Path) -> (Vec<String>, Vec<u8>, Vec<String>) {
    let conn = repo_db(repo).await;
    let rows = conn
        .query_all(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT workspace_id || '|' || repo_id || '|' || kind || '|' || path || '|' || \
             state || '|' || COALESCE(lease_owner, '') || '|' || lease_fence || '|' || \
             COALESCE(lease_expires_at, -1) || '|' || updated_at AS row FROM workspace_record \
             ORDER BY workspace_id",
        ))
        .await
        .expect("dump workspace rows");
    let rows: Vec<String> = rows
        .iter()
        .map(|row| row.try_get_by_index::<String>(0).expect("row text"))
        .collect();

    let registry = fs::read(repo.join(".libra/worktrees.json")).unwrap_or_default();

    let mut tree = Vec::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.to_string_lossy().to_string();
            if name.ends_with("-wal")
                || name.ends_with("-shm")
                || name.ends_with("-journal")
                || name.ends_with(".lock")
                || name.ends_with("object-index-repair-locks")
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path.clone());
            }
            tree.push(name);
        }
    }
    tree.sort();
    (rows, registry, tree)
}

fn parse_json(bytes: &[u8], what: &str) -> serde_json::Value {
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(text.trim()).unwrap_or_else(|error| panic!("parse {what} json: {error}"))
}

/// The doctor reports what is wrong with each workspace scope — and changes
/// nothing while doing so (§C.8 W4 acceptance: default invocation is strictly
/// read-only).
#[tokio::test]
#[serial]
async fn worktree_doctor_reports_scope_diagnostics() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    // A healthy, currently-leased workspace...
    let healthy = seed_linked_workspace(repo.path(), "live", now_ms()).await;
    // ...and one whose lease expired long ago and whose directory is gone.
    let broken = seed_linked_workspace(repo.path(), "stale", 1_000).await;
    fs::remove_dir_all(repo.path().join("stale")).expect("drop the stale workspace directory");

    let before = repo_snapshot(repo.path()).await;
    let output = run_libra_command(&["--json", "worktree", "doctor"], repo.path());
    assert_cli_success(&output, "worktree doctor");
    let after = repo_snapshot(repo.path()).await;
    assert_eq!(
        before, after,
        "the default doctor invocation must not write"
    );

    let doc = parse_json(&output.stdout, "doctor page");
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["command"], "worktree.doctor", "{doc}");
    assert_eq!(doc["data"]["schema_version"], 1, "{doc}");
    let diagnostics = doc["data"]["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 2, "{doc}");

    let find = |id: &str| {
        diagnostics
            .iter()
            .find(|item| item["workspace_id"] == id)
            .unwrap_or_else(|| panic!("diagnostic for {id} missing: {doc}"))
            .clone()
    };
    let codes = |item: &serde_json::Value| -> Vec<String> {
        item["scope_diagnostics"]
            .as_array()
            .expect("scope_diagnostics array")
            .iter()
            .map(|entry| entry["code"].as_str().expect("code").to_string())
            .collect()
    };

    let healthy_item = find(&healthy);
    assert_eq!(healthy_item["lease_state"], "held", "{healthy_item}");
    // Neither workspace is in the worktree registry (no `worktree add` ran),
    // so both report the missing owner entry — that IS the finding.
    assert!(
        codes(&healthy_item).contains(&"registry_entry_missing".to_string()),
        "{healthy_item}"
    );
    assert!(
        !codes(&healthy_item).contains(&"lease_expired".to_string()),
        "a live lease must not be reported as expired: {healthy_item}"
    );

    let broken_item = find(&broken);
    assert_eq!(broken_item["lease_state"], "expired", "{broken_item}");
    let broken_codes = codes(&broken_item);
    for expected in [
        "lease_expired",
        "workspace_path_missing",
        "registry_entry_missing",
    ] {
        assert!(
            broken_codes.contains(&expected.to_string()),
            "missing {expected} in {broken_item}"
        );
    }
    // Severity is part of the machine contract, not decoration.
    for entry in broken_item["scope_diagnostics"]
        .as_array()
        .expect("scope_diagnostics")
    {
        let severity = entry["severity"].as_str().expect("severity");
        assert!(
            severity == "warning" || severity == "error",
            "unexpected severity {severity}: {broken_item}"
        );
    }
}

/// The W0 read-only skeleton contract, stated on its own: neither the human
/// nor the JSON form of the default invocation may mutate anything.
#[tokio::test]
#[serial]
async fn worktree_doctor_default_invocation_is_readonly() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    let workspace_id = seed_linked_workspace(repo.path(), "scope", now_ms()).await;

    let before = repo_snapshot(repo.path()).await;
    for args in [
        vec!["worktree", "doctor"],
        vec!["--json", "worktree", "doctor"],
        vec!["worktree", "doctor", workspace_id.as_str()],
    ] {
        let output = run_libra_command(&args, repo.path());
        assert_cli_success(&output, "read-only doctor invocation");
        let after = repo_snapshot(repo.path()).await;
        assert_eq!(before, after, "`libra {}` wrote state", args.join(" "));
    }
}

/// Envelope, required fields, ordering, page limits, opaque cursor and both
/// fail-closed refusals — the frozen machine contract (§C.8, Codex R18/R19).
#[tokio::test]
#[serial]
async fn worktree_doctor_json_schema_and_pagination_stable() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    let conn = repo_db(repo.path()).await;
    let repo_id = repo_identity(&conn).await;
    bulk_seed(&conn, &repo_id, 600).await;

    // Default page size is 50; the sort key is `workspace_id` ascending.
    let default_page = run_libra_command(&["--json", "worktree", "doctor"], repo.path());
    assert_cli_success(&default_page, "default doctor page");
    let doc = parse_json(&default_page.stdout, "default page");
    assert_eq!(doc["command"], "worktree.doctor", "{doc}");
    let items = doc["data"]["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(items.len(), 50, "default limit is 50: {doc}");
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["workspace_id"].as_str().expect("id"))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "diagnostics are ordered by workspace_id");
    assert_eq!(ids[0], "bulk-0000", "{doc}");

    // The required field set is frozen schema v1.
    let mut keys: Vec<String> = items[0]
        .as_object()
        .expect("diagnostic object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "kind",
            "lease_expires_at",
            "lease_fence",
            "lease_owner",
            "lease_state",
            "path",
            "repo_id",
            "scope_diagnostics",
            "state",
            "workspace_id",
            "worktree_id",
        ],
        "diagnostic key set is frozen schema v1: {doc}"
    );

    // An over-large --limit is capped at 500, not honoured.
    let capped = run_libra_command(
        &["--json", "worktree", "doctor", "--limit", "5000"],
        repo.path(),
    );
    assert_cli_success(&capped, "capped doctor page");
    let capped_doc = parse_json(&capped.stdout, "capped page");
    assert_eq!(
        capped_doc["data"]["diagnostics"]
            .as_array()
            .expect("capped diagnostics")
            .len(),
        500,
        "page cap is 500"
    );
    assert!(
        capped_doc["data"]["next_cursor"].is_string(),
        "600 rows must leave a next page: {capped_doc}"
    );

    // Keyset walk: the cursor is opaque, and the next page continues strictly
    // after the previous one with no repeats.
    let page1 = run_libra_command(
        &["--json", "worktree", "doctor", "--limit", "2"],
        repo.path(),
    );
    assert_cli_success(&page1, "doctor page 1");
    let doc1 = parse_json(&page1.stdout, "page1");
    let cursor = doc1["data"]["next_cursor"]
        .as_str()
        .expect("cursor")
        .to_string();
    assert_ne!(
        cursor, "bulk-0001",
        "the cursor must be opaque, not a raw workspace_id"
    );
    let page2 = run_libra_command(
        &[
            "--json", "worktree", "doctor", "--limit", "2", "--cursor", &cursor,
        ],
        repo.path(),
    );
    assert_cli_success(&page2, "doctor page 2");
    let doc2 = parse_json(&page2.stdout, "page2");
    let ids2: Vec<&str> = doc2["data"]["diagnostics"]
        .as_array()
        .expect("page2 diagnostics")
        .iter()
        .map(|item| item["workspace_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids2, vec!["bulk-0002", "bulk-0003"], "{doc2}");

    // Single-scope view: singular key, and NO pagination field.
    let single = run_libra_command(&["--json", "worktree", "doctor", "bulk-0007"], repo.path());
    assert_cli_success(&single, "single scope view");
    let single_doc = parse_json(&single.stdout, "single scope");
    assert_eq!(single_doc["command"], "worktree.doctor", "{single_doc}");
    assert_eq!(single_doc["data"]["schema_version"], 1, "{single_doc}");
    assert_eq!(
        single_doc["data"]["diagnostic"]["workspace_id"], "bulk-0007",
        "{single_doc}"
    );
    assert!(
        single_doc["data"].get("next_cursor").is_none(),
        "the single-scope view has no pagination keys: {single_doc}"
    );
    assert!(
        single_doc["data"].get("diagnostics").is_none(),
        "the single-scope view uses the singular key: {single_doc}"
    );

    // A cursor this command did not issue fails closed (LBR-WORKTREE-001)
    // instead of quietly restarting at page one.
    let bad_cursor = run_libra_command(
        &["--json", "worktree", "doctor", "--cursor", "not-a-cursor"],
        repo.path(),
    );
    assert!(!bad_cursor.status.success(), "invalid cursor must fail");
    let bad_doc = parse_json(&bad_cursor.stderr, "invalid cursor error");
    assert_eq!(bad_doc["ok"], false, "{bad_doc}");
    assert_eq!(bad_doc["error_code"], "LBR-WORKTREE-001", "{bad_doc}");
    assert_eq!(bad_doc["category"], "repo", "{bad_doc}");
    assert_eq!(bad_doc["exit_code"], 128, "{bad_doc}");

    // A single scope is not a page: combining the id with --limit/--cursor is
    // a usage error, not a silently-ignored flag.
    for extra in [["--limit", "2"], ["--cursor", cursor.as_str()]] {
        let usage = run_libra_command(
            &[
                "--json",
                "worktree",
                "doctor",
                "bulk-0007",
                extra[0],
                extra[1],
            ],
            repo.path(),
        );
        assert_eq!(usage.status.code(), Some(129), "usage exit for {extra:?}");
        let usage_doc = parse_json(&usage.stderr, "usage error");
        assert_eq!(usage_doc["error_code"], "LBR-CLI-002", "{usage_doc}");
        assert_eq!(usage_doc["category"], "cli", "{usage_doc}");
    }
}

/// An unreadable scope is refused (`LBR-WORKTREE-002`) — the doctor never
/// answers with an empty or partial diagnosis built on unknown ownership.
#[tokio::test]
#[serial]
async fn worktree_doctor_corrupt_scope_fails_closed() {
    let repo = tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());
    seed_linked_workspace(repo.path(), "scope", now_ms()).await;

    fs::write(repo.path().join(".libra/worktrees.json"), b"{ not json")
        .expect("corrupt the worktree registry");

    let output = run_libra_command(&["--json", "worktree", "doctor"], repo.path());
    assert!(!output.status.success(), "corrupt scope must fail closed");
    let doc = parse_json(&output.stderr, "corrupt scope error");
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["error_code"], "LBR-WORKTREE-002", "{doc}");
    assert_eq!(doc["category"], "repo", "{doc}");
}

/// Every `worktree doctor` mention in a user-facing hint must be INSPECT-ONLY
/// while the mutating grammar is unfrozen (§C.11 W0, Codex R19/R20): a hint
/// must not name a recovery command the CLI cannot run.
#[test]
fn worktree_doctor_hints_are_inspect_only() {
    const SOURCES: [(&str, &str); 3] = [
        (
            "workspace.rs",
            include_str!("../../src/internal/workspace.rs"),
        ),
        ("worktree.rs", include_str!("../../src/command/worktree.rs")),
        (
            "environment.rs",
            include_str!("../../src/internal/ai/runtime/environment.rs"),
        ),
    ];
    for (name, source) in SOURCES {
        for (index, line) in source.lines().enumerate() {
            if !line.contains("worktree doctor") {
                continue;
            }
            // Comments explain the rule itself; only user-visible strings and
            // the surrounding sentence of a hint are constrained.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for forbidden in ["reclaim", "adopt", "release"] {
                assert!(
                    !line.contains(forbidden),
                    "{name}:{} promises a `{forbidden}` action the bare `worktree doctor` \
                     cannot perform: {line}",
                    index + 1
                );
            }
        }
    }
}
