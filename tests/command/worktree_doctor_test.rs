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

    // The paginated top-level `data` key set is frozen too: the workspace
    // page plus the W0 worktree-scope half (`worktrees[]`).
    let mut data_keys: Vec<String> = doc["data"]
        .as_object()
        .expect("data object")
        .keys()
        .cloned()
        .collect();
    data_keys.sort();
    assert_eq!(
        data_keys,
        vec!["diagnostics", "next_cursor", "schema_version", "worktrees"],
        "paginated data key set is frozen schema v1: {doc}"
    );
    let mut worktree_keys: Vec<String> = doc["data"]["worktrees"]
        .as_array()
        .expect("worktrees array")
        .first()
        .expect("at least the main worktree is diagnosed")
        .as_object()
        .expect("worktree diagnostic object")
        .keys()
        .cloned()
        .collect();
    worktree_keys.sort();
    assert_eq!(
        worktree_keys,
        vec![
            "findings",
            "identity_registered",
            "is_main",
            "layout",
            "path",
            "state",
            "worktree_id",
        ],
        "worktree diagnostic key set is frozen schema v1: {doc}"
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

// ---------------------------------------------------------------------------
// W0 (§C.11, Codex R16/R17): mutating repair actions require `--confirm` and
// emit exactly one operation-log audit event per executed action.
// ---------------------------------------------------------------------------

/// One worktree directory's full content tree (relative path -> bytes), with
/// a symlink recorded as a `path -> target` entry so a legacy `.libra` link
/// is compared as a link, never followed.
#[cfg(unix)]
fn worktree_dir_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("walked path is beneath its root")
                .to_string_lossy()
                .to_string();
            let file_type = entry.file_type().expect("file type");
            if file_type.is_symlink() {
                let target = fs::read_link(&path).expect("read symlink target");
                entries.push((format!("{relative} -> {}", target.display()), Vec::new()));
            } else if file_type.is_dir() {
                stack.push(path);
            } else {
                entries.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    entries.sort();
    entries
}

/// The repository's operation rows, newest first, read through the public
/// operation service (the same API `libra op log` paginates).
#[cfg(unix)]
async fn operation_rows(repo: &Path) -> Vec<libra::internal::operation::OperationLogListItem> {
    use libra::internal::operation::{OperationQueryPage, OperationService};

    let conn = repo_db(repo).await;
    let repo_id = repo_identity(&conn).await;
    let page = OperationService::list_operations_by_repo_paginated_with_conn(
        &conn,
        &repo_id,
        OperationQueryPage {
            page: 1,
            per_page: 200,
        },
    )
    .await
    .expect("list operation rows");
    let mut rows = page.items;
    rows.sort_by(|a, b| a.op_id.cmp(&b.op_id));
    rows
}

/// W0 (§C.11, Codex R16/R17), table-driven over the three mutating repair
/// actions — `repair <path>`, the no-arg registry repair, and a non-dry-run
/// `--migrate-layout`:
///
/// 1. WITHOUT `--confirm` the action is refused with ZERO side effects: the
///    registry file, the operation table and every watched worktree
///    directory are byte-identical before and after.
/// 2. WITH `--confirm` the action runs, exactly ONE new operation row names
///    it (complete: actor, outcome, finish timestamp), and only the action's
///    target scope changes — a bystander worktree is byte-identical.
///
/// Unix-only because the legacy-symlink layout the migration acts on cannot
/// be built without `symlink(2)`.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn worktree_doctor_mutations_require_confirmation_and_emit_audit() {
    use libra::internal::operation::OperationStatus;

    enum RepairAction {
        IdentityPath,
        RegistryAll,
        MigrateLayout,
    }

    let table = [
        (RepairAction::IdentityPath, "worktree repair <path>"),
        (RepairAction::RegistryAll, "worktree repair"),
        (
            RepairAction::MigrateLayout,
            "worktree repair --migrate-layout",
        ),
    ];

    for (action, command_name) in table {
        let repo = create_committed_repo_via_cli();
        let main = repo.path();

        // Two scopes in every row: the action's target and a bystander the
        // action must never touch.
        let target = main.join("wt-target");
        let bystander = main.join("wt-bystander");
        assert_cli_success(
            &run_libra_command(&["worktree", "add", "wt-bystander"], main),
            "add bystander worktree",
        );

        // Per-row setup: what the action repairs, and its argv.
        let argv: Vec<String> = match action {
            RepairAction::IdentityPath => {
                assert_cli_success(
                    &run_libra_command(&["worktree", "add", "wt-target"], main),
                    "add target worktree",
                );
                fs::remove_file(target.join(".libra").join("worktree_id"))
                    .expect("damage the target's gitdir identity");
                vec!["worktree".into(), "repair".into(), "wt-target".into()]
            }
            RepairAction::RegistryAll => {
                assert_cli_success(
                    &run_libra_command(&["worktree", "add", "wt-target"], main),
                    "add target worktree",
                );
                // A duplicated registry entry: exactly what the no-arg
                // repair exists to remove.
                let registry = main.join(".libra").join("worktrees.json");
                let mut doc: serde_json::Value =
                    serde_json::from_slice(&fs::read(&registry).expect("read registry"))
                        .expect("registry json");
                let duplicate = doc["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .find(|entry| {
                        entry["path"]
                            .as_str()
                            .is_some_and(|path| path.ends_with("wt-target"))
                    })
                    .expect("target entry")
                    .clone();
                doc["entries"]
                    .as_array_mut()
                    .expect("entries")
                    .push(duplicate);
                fs::write(
                    &registry,
                    serde_json::to_vec_pretty(&doc).expect("serialize"),
                )
                .expect("plant the duplicate entry");
                vec!["worktree".into(), "repair".into()]
            }
            RepairAction::MigrateLayout => {
                // The target is a LEGACY shared-.libra symlink worktree;
                // migration replaces its gitdir with a real one.
                fs::create_dir_all(&target).expect("mkdir legacy target");
                std::os::unix::fs::symlink(main.join(".libra"), target.join(".libra"))
                    .expect("legacy symlink");
                // Materialize the registry, then register the legacy entry
                // the way the v1→v2 upgrade would have backfilled it.
                assert_cli_success(
                    &run_libra_command(&["worktree", "repair", "--confirm"], main),
                    "materialize registry",
                );
                let registry = main.join(".libra").join("worktrees.json");
                let mut doc: serde_json::Value =
                    serde_json::from_slice(&fs::read(&registry).expect("read registry"))
                        .expect("registry json");
                let canonical = target.canonicalize().expect("canonical target");
                doc["entries"]
                    .as_array_mut()
                    .expect("entries")
                    .push(serde_json::json!({
                        "path": canonical.to_string_lossy(),
                        "is_main": false,
                        "locked": false,
                        "lock_reason": null,
                        "worktree_id": "legacy-wt-target",
                    }));
                fs::write(
                    &registry,
                    serde_json::to_vec_pretty(&doc).expect("serialize"),
                )
                .expect("register the legacy entry");
                vec![
                    "worktree".into(),
                    "repair".into(),
                    "--migrate-layout".into(),
                ]
            }
        };

        // ---- Phase 1: no `--confirm` -> refused, ZERO side effects. ----
        let registry_path = main.join(".libra").join("worktrees.json");
        let registry_before = fs::read(&registry_path).expect("registry before");
        let operations_before = operation_rows(main).await;
        let target_tree_before = worktree_dir_tree(&target);
        let bystander_tree_before = worktree_dir_tree(&bystander);

        let refused = run_libra_command(&argv.iter().map(String::as_str).collect::<Vec<_>>(), main);
        assert!(
            !refused.status.success(),
            "{command_name} without --confirm must be refused: {}",
            String::from_utf8_lossy(&refused.stdout)
        );
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.contains("without confirmation; re-run with --confirm"),
            "{command_name}: the refusal names the required flag: {stderr}"
        );
        assert_eq!(
            fs::read(&registry_path).expect("registry after refusal"),
            registry_before,
            "{command_name}: the refusal must not touch the registry"
        );
        assert_eq!(
            operation_rows(main).await,
            operations_before,
            "{command_name}: the refusal must not write an operation row"
        );
        assert_eq!(
            worktree_dir_tree(&target),
            target_tree_before,
            "{command_name}: the refusal must not touch the target worktree"
        );
        assert_eq!(
            worktree_dir_tree(&bystander),
            bystander_tree_before,
            "{command_name}: the refusal must not touch the bystander worktree"
        );

        // ---- Phase 2: `--confirm` -> runs, exactly one audit row, only the
        // target scope modified. ----
        let mut confirmed = argv.clone();
        confirmed.push("--confirm".into());
        let ran = run_libra_command(
            &confirmed.iter().map(String::as_str).collect::<Vec<_>>(),
            main,
        );
        assert_cli_success(&ran, "confirmed repair action");

        let operations_after = operation_rows(main).await;
        assert_eq!(
            operations_after.len(),
            operations_before.len() + 1,
            "{command_name}: exactly one audit row per executed action"
        );
        let new_rows: Vec<_> = operations_after
            .iter()
            .filter(|after| {
                !operations_before
                    .iter()
                    .any(|before| before.op_id == after.op_id)
            })
            .collect();
        assert_eq!(new_rows.len(), 1, "{command_name}: the audit row is unique");
        let audit = new_rows[0];
        assert_eq!(
            audit.command_name, command_name,
            "the audit row names the action"
        );
        assert_eq!(
            audit.status,
            OperationStatus::Succeeded,
            "the audit row records the outcome"
        );
        assert!(
            audit.description.contains(command_name),
            "the audit row describes the action: {}",
            audit.description
        );
        assert!(!audit.actor.is_empty(), "the audit row names an actor");
        assert!(
            audit.end_ts.is_some(),
            "the audit row is finished, not left running"
        );

        // Only the target scope changed.
        assert_eq!(
            worktree_dir_tree(&bystander),
            bystander_tree_before,
            "{command_name}: the bystander worktree is untouched"
        );
        match action {
            RepairAction::IdentityPath => {
                // The target's identity was restored FROM THE REGISTRY, and
                // the registry itself was not rewritten.
                let restored = fs::read_to_string(target.join(".libra").join("worktree_id"))
                    .expect("identity restored");
                let doc: serde_json::Value =
                    serde_json::from_slice(&fs::read(&registry_path).expect("registry"))
                        .expect("registry json");
                let persisted = doc["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .find(|entry| {
                        entry["path"]
                            .as_str()
                            .is_some_and(|path| path.ends_with("wt-target"))
                    })
                    .and_then(|entry| entry["worktree_id"].as_str())
                    .expect("persisted id");
                assert_eq!(
                    restored.trim(),
                    persisted,
                    "the restored identity is the registry's persisted id"
                );
                assert_eq!(
                    fs::read(&registry_path).expect("registry after repair"),
                    registry_before,
                    "identity repair touches the target gitdir only"
                );
            }
            RepairAction::RegistryAll => {
                // The registry (this action's target scope) was healed: the
                // duplicate is gone and neither worktree directory moved.
                let healed = fs::read(&registry_path).expect("registry after repair");
                assert_ne!(healed, registry_before, "the duplicate was removed");
                let doc: serde_json::Value =
                    serde_json::from_slice(&healed).expect("registry json");
                let target_entries = doc["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .filter(|entry| {
                        entry["path"]
                            .as_str()
                            .is_some_and(|path| path.ends_with("wt-target"))
                    })
                    .count();
                assert_eq!(target_entries, 1, "the duplicate entry is gone");
                assert_eq!(
                    worktree_dir_tree(&target),
                    target_tree_before,
                    "registry repair touches no worktree directory"
                );
            }
            RepairAction::MigrateLayout => {
                // The legacy target (this action's target scope) now has a
                // REAL gitdir; the registry's SEMANTIC content is unchanged
                // (compared as parsed JSON — the migration's save normalizes
                // key order, which is not a scope mutation).
                assert!(
                    fs::symlink_metadata(target.join(".libra"))
                        .expect("gitdir metadata")
                        .file_type()
                        .is_dir(),
                    "the legacy symlink was replaced by a real gitdir"
                );
                let after: serde_json::Value = serde_json::from_slice(
                    &fs::read(&registry_path).expect("registry after migration"),
                )
                .expect("registry after migration parses");
                let before: serde_json::Value =
                    serde_json::from_slice(&registry_before).expect("registry before parses");
                assert_eq!(
                    after, before,
                    "migration touches the target gitdir only (registry semantics)"
                );
            }
        }
    }
}

/// W0 (§C.11, Codex R18): `repair <path> --resolve-identity` keeps its
/// dedicated `--yes` confirmation, but once confirmed the detach runs inside
/// the SAME one-row operation-log audit boundary as every other mutating
/// repair — the `--yes`-less refusal writes no row, a success closes its row
/// `succeeded`, and a nothing-to-resolve failure closes its row `failed`.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn worktree_repair_resolve_identity_runs_inside_the_audit_boundary() {
    use libra::internal::operation::OperationStatus;

    let repo = create_committed_repo_via_cli();
    let main = repo.path();

    // Two live worktrees, then handcraft the legacy-binary identity
    // collision: BOTH entries claim wt-b's identity.
    assert_cli_success(
        &run_libra_command(&["worktree", "add", "wt-a"], main),
        "add worktree a",
    );
    assert_cli_success(
        &run_libra_command(&["worktree", "add", "wt-b"], main),
        "add worktree b",
    );
    let registry_path = main.join(".libra").join("worktrees.json");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).expect("read registry"))
            .expect("registry json");
    let identity_b = doc["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("wt-b"))
        })
        .and_then(|entry| entry["worktree_id"].as_str())
        .expect("wt-b identity")
        .to_string();
    doc["entries"]
        .as_array_mut()
        .expect("entries")
        .iter_mut()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("wt-a"))
        })
        .expect("wt-a entry")["worktree_id"] = serde_json::Value::String(identity_b);
    fs::write(
        &registry_path,
        serde_json::to_vec_pretty(&doc).expect("serialize"),
    )
    .expect("plant the identity collision");

    // 1. Without `--yes` the action is refused and writes NO operation row.
    let operations_before = operation_rows(main).await;
    let registry_before = fs::read(&registry_path).expect("registry before");
    let refused = run_libra_command(&["worktree", "repair", "wt-a", "--resolve-identity"], main);
    assert!(
        !refused.status.success(),
        "resolve-identity without --yes must be refused"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("--yes"),
        "the refusal names its dedicated confirmation: {stderr}"
    );
    assert_eq!(
        operation_rows(main).await,
        operations_before,
        "the refusal must not write an operation row"
    );
    assert_eq!(
        fs::read(&registry_path).expect("registry after refusal"),
        registry_before,
        "the refusal must not touch the registry"
    );

    // 2. `--yes` detaches the named entry and closes exactly ONE new audit
    //    row `succeeded`, named after the action.
    let ran = run_libra_command(
        &["worktree", "repair", "wt-a", "--resolve-identity", "--yes"],
        main,
    );
    assert_cli_success(&ran, "resolve the identity collision");
    let operations_after = operation_rows(main).await;
    assert_eq!(
        operations_after.len(),
        operations_before.len() + 1,
        "exactly one audit row per executed action"
    );
    let audit = operations_after
        .iter()
        .find(|after| {
            !operations_before
                .iter()
                .any(|before| before.op_id == after.op_id)
        })
        .expect("the new audit row");
    assert_eq!(
        audit.command_name, "worktree repair wt-a --resolve-identity",
        "the audit row names the action"
    );
    assert_eq!(
        audit.status,
        OperationStatus::Succeeded,
        "the audit row records the outcome"
    );
    assert!(
        audit.end_ts.is_some(),
        "the audit row is finished, not left running"
    );

    // ... and the collision is actually resolved: wt-a is detached, so wt-b
    // owns the identity alone again.
    let healed: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).expect("registry after repair"))
            .expect("registry json");
    let state_a = healed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("wt-a"))
        })
        .and_then(|entry| entry["state"].as_str());
    assert_eq!(
        state_a,
        Some("detached_from_registry"),
        "the named entry was detached"
    );

    // 3. A resolve with NOTHING left to resolve fails — and its audit row
    //    closes `failed` (the boundary records the outcome either way).
    let failed = run_libra_command(
        &["worktree", "repair", "wt-b", "--resolve-identity", "--yes"],
        main,
    );
    assert!(
        !failed.status.success(),
        "wt-b has no collision left: the resolve must fail"
    );
    let operations_final = operation_rows(main).await;
    assert_eq!(
        operations_final.len(),
        operations_after.len() + 1,
        "the failure is audited too"
    );
    let failed_row = operations_final
        .iter()
        .find(|row| {
            !operations_after
                .iter()
                .any(|before| before.op_id == row.op_id)
        })
        .expect("the failure audit row");
    assert_eq!(
        failed_row.command_name, "worktree repair wt-b --resolve-identity",
        "the failure row names the attempted action"
    );
    assert_eq!(failed_row.status, OperationStatus::Failed);
    assert!(failed_row.end_ts.is_some());
}

/// W0 (§C.11, Codex R18 follow-up): every user-visible `libra worktree
/// repair` suggestion in the crate source names its confirmation inside the
/// same backticks — `--confirm` for the ordinary mutating actions, `--yes`
/// for `--resolve-identity`, `--dry-run` for the read-only preview. A bare
/// suggestion refuses deterministically (W0 repair gate), so printing one
/// sends the user to a second failure at the moment they are already stuck.
/// This scan keeps hints, doctor findings and recovery output honest.
///
/// Rust string literals treat `\` + newline (+ leading whitespace) as a line
/// continuation — the RENDERED string joins the pieces, so a hint split that
/// way would hide from a raw-source search. The scan normalizes the source
/// the same way the compiler does before looking for spans.
///
/// The C.3.3 documentation sources that prescribe repair invocations are
/// held to the same contract (Codex R19 follow-up): a developer doc showing
/// a bare command sends its reader to the same refusal. COMPATIBILITY.md
/// and the migration SQL comments prescribe them too (Codex W0-r7
/// follow-up); they spell the command without the `libra` prefix, so they
/// are scanned with the bare `worktree repair` needle, and a span
/// explicitly attributed to Git's own CLI is exempt.
#[test]
fn repair_guidance_in_source_always_names_its_confirmation() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    let mut stack = vec![manifest.join("src")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read src directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read source file");
            let text = join_rust_string_continuations(&text);
            scan_repair_spans(&path, &text, "libra worktree repair", &mut violations);
        }
    }
    for doc in [
        "docs/development/libra-worktree-architecture.md",
        "docs/development/commands/worktree.md",
    ] {
        let path = manifest.join(doc);
        let text = fs::read_to_string(&path).expect("read doc file");
        scan_repair_spans(&path, &text, "libra worktree repair", &mut violations);
    }
    // COMPATIBILITY.md and the migration SQL comments also prescribe repair
    // invocations (Codex W0-r7 follow-up). They spell the command without
    // the `libra` prefix, so they are scanned with the bare needle.
    let compat = manifest.join("COMPATIBILITY.md");
    let text = fs::read_to_string(&compat).expect("read COMPATIBILITY.md");
    scan_repair_spans(&compat, &text, "worktree repair", &mut violations);
    let mut migration_files: Vec<_> = fs::read_dir(manifest.join("sql/migrations"))
        .expect("read sql/migrations")
        .map(|entry| entry.expect("migration entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sql"))
        .collect();
    migration_files.sort();
    for path in migration_files {
        let text = fs::read_to_string(&path).expect("read migration file");
        scan_repair_spans(&path, &text, "worktree repair", &mut violations);
    }
    assert!(
        violations.is_empty(),
        "repair guidance that would refuse as written:\n{}",
        violations.join("\n")
    );
}

/// Flag every backtick-quoted span in `text` that contains `needle` (a
/// `worktree repair` command spelling) yet names none of the confirmation
/// flags. Sources that prescribe Libra's own CLI are scanned with
/// "libra worktree repair"; sources that spell the command without the
/// prefix (COMPATIBILITY.md, migration SQL comments) use the bare
/// "worktree repair" needle. A span explicitly attributed to Git's own CLI
/// (its opening backtick immediately preceded by "Git's ") is exempt —
/// upstream has no `--confirm`.
fn scan_repair_spans(path: &Path, text: &str, needle: &str, violations: &mut Vec<String>) {
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let span_start = text.len() - rest.len() + open + 1;
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            break;
        };
        let span = &rest[..close];
        if span.contains(needle)
            && !span.contains("--confirm")
            && !span.contains("--yes")
            && !span.contains("--dry-run")
            && !text[..span_start].ends_with("Git's `")
        {
            violations.push(format!("{}: `{span}`", path.display()));
        }
        rest = &rest[close + 1..];
    }
}

/// Join Rust string-literal line continuations the way the compiler renders
/// them: a `\` immediately followed by a newline drops the newline and the
/// next line's leading indentation. Without this a hint written as
/// `"... worktree \`<newline>`repair ..."` would stay invisible to the scan.
fn join_rust_string_continuations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("\\\n") {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + 2..];
        rest = rest.trim_start_matches([' ', '\t']);
    }
    out.push_str(rest);
    out
}

/// W0 (§C.11, Codex R19 follow-up): an UNCONFIRMED mutating repair applies NO
/// pending schema migration on its way to the refusal. The refusal is
/// documented as byte-for-byte side-effect free, and a migration is an
/// irreversible write — a bare `worktree repair` run against an older
/// repository must not upgrade its schema while saying no.
#[tokio::test]
async fn unconfirmed_repair_applies_no_pending_migrations() {
    let repo = create_committed_repo_via_cli();
    let main = repo.path();

    // Simulate a pending migration: drop the newest applied-migration row.
    // The migration's own DDL is idempotent (INSERT ... ON CONFLICT DO
    // NOTHING), so the positive control below can cleanly re-apply it.
    let newest: i64 = {
        let conn = repo_db(main).await;
        let row = conn
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT MAX(version) FROM schema_versions".to_string(),
            ))
            .await
            .expect("query newest schema version")
            .expect("at least one applied migration");
        let version = row.try_get_by_index::<i64>(0).expect("version value");
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM schema_versions WHERE version = ?",
            [version.into()],
        ))
        .await
        .expect("drop the newest migration row");
        version
    };

    // The unconfirmed invocation is refused — and the row must STAY deleted.
    let refused = run_libra_command(&["worktree", "repair"], main);
    assert_ne!(
        refused.status.code(),
        Some(0),
        "unconfirmed repair must be refused"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("--confirm"),
        "the refusal names --confirm: {stderr}"
    );
    let remaining: i64 = {
        let conn = repo_db(main).await;
        conn.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) FROM schema_versions WHERE version = ?",
            [newest.into()],
        ))
        .await
        .expect("re-query the dropped row")
        .expect("count row")
        .try_get_by_index::<i64>(0)
        .expect("count value")
    };
    assert_eq!(
        remaining, 0,
        "the unconfirmed refusal must not apply the pending migration"
    );

    // Positive control: an ordinary (migration-applying) worktree command
    // DOES bring the schema current, proving the probe can see application.
    assert_cli_success(
        &run_libra_command(&["worktree", "list"], main),
        "worktree list",
    );
    let reapplied: i64 = {
        let conn = repo_db(main).await;
        conn.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) FROM schema_versions WHERE version = ?",
            [newest.into()],
        ))
        .await
        .expect("re-query after the control command")
        .expect("count row")
        .try_get_by_index::<i64>(0)
        .expect("count value")
    };
    assert_eq!(
        reapplied, 1,
        "the migration-applying control re-applied the pending migration"
    );
}

/// Codex R19 gate finding (round 7): a BARE `worktree doctor` must not apply
/// pending migrations either — the §C.11 guarantee is that doctor is the one
/// command safe to run on a repository you have not yet decided to upgrade.
/// The regression hole this pins: with linked-worktree history present, the
/// doctor's adopted-provenance probe (`adopted_scope_settings_present`)
/// resolved the GLOBAL (migration-applying) connection instead of doctor's
/// no-migration one.
#[tokio::test]
async fn bare_doctor_applies_no_pending_migrations() {
    let repo = create_committed_repo_via_cli();
    let main = repo.path();
    // Linked-worktree history is required to reach the adopted-provenance probe.
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );

    // Simulate a pending migration: drop the newest applied-migration row (the
    // same simulation as `unconfirmed_repair_applies_no_pending_migrations`).
    let newest: i64 = {
        let conn = repo_db(main).await;
        let row = conn
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT MAX(version) FROM schema_versions".to_string(),
            ))
            .await
            .expect("query newest schema version")
            .expect("at least one applied migration");
        let version = row.try_get_by_index::<i64>(0).expect("version value");
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM schema_versions WHERE version = ?",
            [version.into()],
        ))
        .await
        .expect("drop the newest migration row");
        version
    };
    let count_row = async |version: i64| -> i64 {
        let conn = repo_db(main).await;
        conn.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) FROM schema_versions WHERE version = ?",
            [version.into()],
        ))
        .await
        .expect("re-query the dropped row")
        .expect("count row")
        .try_get_by_index::<i64>(0)
        .expect("count value")
    };

    // Bare doctor must succeed AND leave the row dropped.
    assert_cli_success(
        &run_libra_command(&["worktree", "doctor"], main),
        "bare worktree doctor",
    );
    assert_eq!(
        count_row(newest).await,
        0,
        "bare doctor must not apply the pending migration"
    );

    // The JSON surface shares the same no-migration path.
    assert_cli_success(
        &run_libra_command(&["--json", "worktree", "doctor"], main),
        "--json worktree doctor",
    );
    assert_eq!(
        count_row(newest).await,
        0,
        "--json doctor must not apply the pending migration"
    );

    // Positive control: an ordinary (migration-applying) worktree command DOES
    // bring the schema current, proving the probe can see application.
    assert_cli_success(
        &run_libra_command(&["worktree", "list"], main),
        "worktree list",
    );
    assert_eq!(
        count_row(newest).await,
        1,
        "the migration-applying control re-applied the pending migration"
    );
}

/// Codex R20 gate finding (round 8): the read-only `worktree repair
/// --migrate-layout --dry-run` preview must not apply pending migrations
/// either — it is documented as confirmation-free and read-only end to end,
/// and applying migrations is a WRITE. The regression hole this pins: the
/// preview took the migration-applying CLI preflight and dispatch-time opens,
/// and `migrate_layout_run` resolved the GLOBAL (migration-applying)
/// connection before returning its plan.
#[tokio::test]
async fn migrate_layout_dry_run_applies_no_pending_migrations() {
    let repo = create_committed_repo_via_cli();
    let main = repo.path();

    // Simulate a pending migration: drop the newest applied-migration row
    // (the same simulation as `bare_doctor_applies_no_pending_migrations`).
    let newest: i64 = {
        let conn = repo_db(main).await;
        let row = conn
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT MAX(version) FROM schema_versions".to_string(),
            ))
            .await
            .expect("query newest schema version")
            .expect("at least one applied migration");
        let version = row.try_get_by_index::<i64>(0).expect("version value");
        conn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM schema_versions WHERE version = ?",
            [version.into()],
        ))
        .await
        .expect("drop the newest migration row");
        version
    };
    let count_row = async |version: i64| -> i64 {
        let conn = repo_db(main).await;
        conn.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) FROM schema_versions WHERE version = ?",
            [version.into()],
        ))
        .await
        .expect("re-query the dropped row")
        .expect("count row")
        .try_get_by_index::<i64>(0)
        .expect("count value")
    };

    // The dry-run preview must succeed AND leave the row dropped.
    assert_cli_success(
        &run_libra_command(
            &["worktree", "repair", "--migrate-layout", "--dry-run"],
            main,
        ),
        "worktree repair --migrate-layout --dry-run",
    );
    assert_eq!(
        count_row(newest).await,
        0,
        "the dry-run layout preview must not apply the pending migration"
    );

    // The JSON surface shares the same no-migration path.
    assert_cli_success(
        &run_libra_command(
            &[
                "--json",
                "worktree",
                "repair",
                "--migrate-layout",
                "--dry-run",
            ],
            main,
        ),
        "--json worktree repair --migrate-layout --dry-run",
    );
    assert_eq!(
        count_row(newest).await,
        0,
        "the --json dry-run layout preview must not apply the pending migration"
    );

    // Positive control: an ordinary (migration-applying) worktree command DOES
    // bring the schema current, proving the probe can see application.
    assert_cli_success(
        &run_libra_command(&["worktree", "list"], main),
        "worktree list",
    );
    assert_eq!(
        count_row(newest).await,
        1,
        "the migration-applying control re-applied the pending migration"
    );
}

/// Codex R21 gate finding (round 9): the read-only repair modes — the
/// `--migrate-layout --dry-run` preview and every UNCONFIRMED (would-be-
/// refused) repair — must not create `.libra/maintenance.lock` either: the
/// shared maintenance hold creates the file when absent, and a filesystem
/// write breaks the byte-for-byte side-effect-free contract the preview and
/// the refusals are documented to keep. The regression hole this pins:
/// `command_scope` classified every `Repair` invocation as a Repository
/// writer, so the generic shared hold ran before dispatch.
#[tokio::test]
async fn read_only_repair_modes_create_no_maintenance_lock() {
    let repo = create_committed_repo_via_cli();
    let main = repo.path();
    let lock = main.join(".libra").join("maintenance.lock");
    // The setup commands legitimately took the shared hold and created the
    // file; start the probe from a genuinely lock-free repository.
    if lock.exists() {
        std::fs::remove_file(&lock).expect("remove the setup-created maintenance lock");
    }
    let assert_lock_absent = |context: &str| {
        assert!(
            !lock.exists(),
            "{context} must not create .libra/maintenance.lock"
        );
    };

    // The read-only layout preview: plain and --json.
    assert_cli_success(
        &run_libra_command(
            &["worktree", "repair", "--migrate-layout", "--dry-run"],
            main,
        ),
        "worktree repair --migrate-layout --dry-run",
    );
    assert_lock_absent("the dry-run layout preview");
    assert_cli_success(
        &run_libra_command(
            &[
                "--json",
                "worktree",
                "repair",
                "--migrate-layout",
                "--dry-run",
            ],
            main,
        ),
        "--json worktree repair --migrate-layout --dry-run",
    );
    assert_lock_absent("the --json dry-run layout preview");

    // Every unconfirmed-repair variant is refused — and creates nothing.
    for (argv, context) in [
        (vec!["worktree", "repair"], "unconfirmed bare repair"),
        (
            vec!["worktree", "repair", "--migrate-layout"],
            "unconfirmed migrate-layout",
        ),
        (
            vec!["worktree", "repair", "wt"],
            "unconfirmed identity repair",
        ),
        (
            vec!["worktree", "repair", "wt", "--resolve-identity"],
            "unconfirmed resolve-identity",
        ),
    ] {
        let refused = run_libra_command(&argv, main);
        assert_ne!(
            refused.status.code(),
            Some(0),
            "{context} must be refused without its confirmation"
        );
        assert_lock_absent(context);
    }

    // Positive control: a CONFIRMED mutating repair takes the shared hold and
    // creates the lock file, proving the probe can see creation.
    assert_cli_success(
        &run_libra_command(&["worktree", "repair", "--confirm"], main),
        "worktree repair --confirm",
    );
    assert!(
        lock.exists(),
        "the confirmed repair must take the shared maintenance hold"
    );
}
