//! W3-10 / plan-20260714 §C.8 W4 named regressions for Code control sidecar
//! file posture: atomic `control.json` writes at `0600`, and cross-worktree
//! scope mismatch fail-closed.

use std::path::{Path, PathBuf};

use libra::command::code_control_files::{
    CONTROL_INFO_VERSION, ControlInfo, ControlScope, ControlScopeError, ControlScopePolicy,
    ensure_scope_takeover_allowed, resolve_control_paths, write_control_info,
};
use serde_json::json;

use super::*;

fn repo_with_linked(branch: &str) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let repo = create_committed_repo_via_cli();
    let main = repo.path();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], main),
        "feature branch",
    );
    let parent = tempfile::tempdir().expect("wt parent");
    assert_cli_success(
        &run_libra_command(&["branch", branch, "feature"], main),
        "branch for the worktree",
    );
    let wt = parent.path().join(branch);
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap(), branch], main),
        "worktree add",
    );
    (repo, parent, wt)
}

fn repository_id(main: &Path) -> String {
    let out = run_libra_command(&["config", "get", "libra.repoid"], main);
    assert_cli_success(&out, "config get libra.repoid");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn worktree_id(wt: &Path) -> String {
    fs::read_to_string(wt.join(".libra").join("worktree_id"))
        .expect("linked worktree records its id")
        .trim()
        .to_string()
}

/// §C.12 `code_control_info_scope_mismatch_rejected` (W3-10): planting a
/// foreign-worktree `control.json` must not be reclaimable from another
/// worktree, and successful writes must be atomic owner-only files.
#[test]
fn code_control_info_scope_mismatch_rejected() {
    let (repo, _parent, wt) = repo_with_linked("code-ctrl-wt");
    let main = repo.path();
    let repo_id = repository_id(main);
    let linked_id = worktree_id(&wt);

    let main_paths = resolve_control_paths(main, None, None);
    let linked_paths = resolve_control_paths(&wt, None, None);
    assert_ne!(
        main_paths.info, linked_paths.info,
        "default control.json paths must be per-worktree local-gitdir isolated"
    );

    // Foreign (linked) scope planted into MAIN's control path with a dead pid.
    let foreign = json!({
        "version": 2,
        "mode": "write",
        "pid": u32::MAX,
        "baseUrl": "http://127.0.0.1:1",
        "workingDir": wt.to_string_lossy(),
        "startedAt": "2026-01-01T00:00:00Z",
        "repoId": repo_id,
        "worktreeId": linked_id,
    });
    let foreign_bytes = serde_json::to_vec_pretty(&foreign).expect("serialize foreign");
    fs::create_dir_all(main_paths.info.parent().expect("parent")).expect("control dir");
    fs::write(&main_paths.info, &foreign_bytes).expect("plant foreign control.json");

    let main_scope = ControlScope {
        repo_id: repo_id.clone(),
        worktree_id: None,
        workspace_id: None,
        lease_fence: None,
    };
    let storage = main.join(".libra");
    let refused = ensure_scope_takeover_allowed(
        &main_paths.info,
        &main_scope,
        ControlScopePolicy::Worktree,
        true,
        &storage,
    );
    let err = refused.expect_err("cross-worktree control.json must fail closed");
    assert!(
        matches!(err, ControlScopeError::Foreign { .. }),
        "refusal must name a foreign scope: {err:?}"
    );
    assert!(
        err.to_string().contains("CONTROL_SCOPE_CONFLICT"),
        "user-facing refusal must carry CONTROL_SCOPE_CONFLICT: {err}"
    );
    assert_eq!(
        fs::read(&main_paths.info).expect("foreign file survives"),
        foreign_bytes,
        "scope mismatch must leave the foreign sidecar byte-identical"
    );

    // Atomic owner-only write on a clean path (half-written seed replaced fully).
    let clean = main_paths
        .info
        .parent()
        .expect("parent")
        .join("control-clean.json");
    fs::write(&clean, "{truncated").expect("plant truncated seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&clean, fs::Permissions::from_mode(0o644)).expect("widen seed");
    }
    let stamped = ControlInfo {
        version: CONTROL_INFO_VERSION,
        mode: "write".to_string(),
        pid: std::process::id(),
        base_url: "http://127.0.0.1:3000".to_string(),
        mcp_url: None,
        working_dir: main.to_path_buf(),
        thread_id: None,
        started_at: chrono::Utc::now(),
        repo_id: Some(repo_id),
        worktree_id: None,
        workspace_id: None,
        lease_fence: None,
        pid_starttime: None,
    };
    write_control_info(&clean, &stamped).expect("atomic write");
    let body = fs::read_to_string(&clean).expect("read stamped control.json");
    assert!(
        !body.contains("truncated"),
        "atomic write must not leave a half-written final path: {body}"
    );
    let parsed: ControlInfo = serde_json::from_str(&body).expect("complete JSON");
    assert_eq!(parsed.base_url, "http://127.0.0.1:3000");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&clean).expect("meta").permissions().mode() & 0o777,
            0o600,
            "control.json must be 0600 after write_control_info"
        );
    }
}
