//! W4-02 / W4-10: canonical `libra code --control stdio` JSON-RPC client.
//!
//! L1 — deterministic. Covers CLI conflict surface via the real binary,
//! control-info discovery/attach fail-closed (W4-10), and attach/submit/detach
//! through the shared
//! [`libra::command::code_control::run_control_stdio_client`] helper (the
//! transport formerly also exposed by the W4-09 shim removed in W5-01).

use std::{fs, io::Write, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

use super::*;

#[cfg(unix)]
fn write_control_token(path: &std::path::Path, token: &str) {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    // Remove first so create+mode(0o600) applies; truncating an existing
    // world-readable file would keep the wide mode.
    let _ = fs::remove_file(path);
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .expect("create token")
        .write_all(token.as_bytes())
        .expect("write token");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod 0600");
}

#[cfg(not(unix))]
fn write_control_token(path: &std::path::Path, token: &str) {
    fs::write(path, token).expect("write token");
}

#[test]
fn code_control_stdio_help_parses_control_mode() {
    let repo = tempdir().expect("tempdir");
    let output = run_libra_command(&["code", "--control", "stdio", "--help"], repo.path());
    assert!(
        output.status.success(),
        "`libra code --control stdio --help` must parse; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn code_control_stdio_rejects_missing_discovery_without_overrides() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["code", "--control", "stdio"], repo.path());
    assert!(
        !output.status.success(),
        "missing control.json must fail closed without --control-url"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CONTROL_INFO_MISSING"),
        "usage must surface CONTROL_INFO_MISSING; stderr={stderr}"
    );
}

#[test]
fn code_control_stdio_rejects_mcp_stdio_combination() {
    let repo = tempdir().expect("tempdir");
    let output = run_libra_command(
        &[
            "code",
            "--control",
            "stdio",
            "--stdio",
            "--control-url",
            "http://127.0.0.1:3000",
            "--control-token-file",
            "token",
        ],
        repo.path(),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MCP") && stderr.contains("--control stdio"),
        "must distinguish MCP --stdio from --control stdio; stderr={stderr}"
    );
}

#[test]
fn code_control_stdio_rejects_non_loopback_control_url() {
    let repo = tempdir().expect("tempdir");
    let token = repo.path().join("control-token");
    write_control_token(&token, "process-token\n");
    let output = run_libra_command(
        &[
            "code",
            "--control",
            "stdio",
            "--control-url",
            "https://evil.example",
            "--control-token-file",
            token.to_str().expect("utf8"),
        ],
        repo.path(),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("loopback"),
        "non-loopback --control-url must fail closed; stderr={stderr}"
    );
}

#[test]
fn code_control_stdio_rejects_single_dash_control() {
    let repo = tempdir().expect("tempdir");
    let output = run_libra_command(&["code", "-control", "stdio"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--control stdio") && stderr.contains("-control"),
        "single-dash -control must point at --control stdio; stderr={stderr}"
    );
}

/// W4-10 Verification: default/override precedence + fail-closed discovery/attach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_stdio_discovery_contract() {
    use libra::command::code_control_files::{
        CONTROL_INFO_VERSION, ControlInfo, current_pid_starttime, resolve_control_paths,
        write_control_info,
    };

    let repo = create_committed_repo_via_cli();
    let main = repo.path();
    let repo_id = {
        let out = run_libra_command(&["config", "get", "libra.repoid"], main);
        assert_cli_success(&out, "config get libra.repoid");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let paths = resolve_control_paths(main, None, None);
    fs::create_dir_all(paths.info.parent().expect("parent")).expect("control dir");

    // --- missing control.json ---
    let missing = run_libra_command(&["code", "--control", "stdio"], main);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("CONTROL_INFO_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&missing.stderr)
    );
    let machine = run_libra_command(&["--machine", "code", "--control", "stdio"], main);
    assert!(!machine.status.success());
    let machine_stderr = String::from_utf8_lossy(&machine.stderr);
    assert!(
        machine_stderr.contains("\"code\":\"CONTROL_INFO_MISSING\"")
            || machine_stderr.contains("CONTROL_INFO_MISSING"),
        "--machine must preserve CONTROL_* in details; stderr={machine_stderr}"
    );

    // Plant live discovery + 0600 token.
    let info = ControlInfo {
        version: CONTROL_INFO_VERSION,
        mode: "write".to_string(),
        pid: std::process::id(),
        base_url: "http://127.0.0.1:3999".to_string(),
        mcp_url: None,
        working_dir: main.to_path_buf(),
        thread_id: None,
        started_at: chrono::Utc::now(),
        repo_id: Some(repo_id.clone()),
        worktree_id: None,
        workspace_id: None,
        lease_fence: None,
        pid_starttime: current_pid_starttime(),
    };
    write_control_info(&paths.info, &info).expect("write control.json");
    write_control_token(&paths.token, "discovered-token\n");

    // --- dead server PID ---
    let mut dead = info.clone();
    dead.pid = u32::MAX;
    dead.base_url = "http://127.0.0.1:1".to_string();
    write_control_info(&paths.info, &dead).expect("dead pid info");
    let dead_out = run_libra_command(&["code", "--control", "stdio"], main);
    assert!(!dead_out.status.success());
    assert!(
        String::from_utf8_lossy(&dead_out.stderr).contains("CONTROL_SERVER_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&dead_out.stderr)
    );

    // Restore live info for remaining cases.
    write_control_info(&paths.info, &info).expect("restore live info");

    // --- PID-reuse: live pid with mismatched starttime ---
    #[cfg(target_os = "linux")]
    {
        let mut reused = info.clone();
        reused.pid_starttime = Some(0);
        write_control_info(&paths.info, &reused).expect("mismatched starttime");
        let reuse = run_libra_command(&["code", "--control", "stdio"], main);
        assert!(!reuse.status.success());
        assert!(
            String::from_utf8_lossy(&reuse.stderr).contains("CONTROL_SERVER_MISSING"),
            "PID-reuse starttime mismatch must fail closed; stderr={}",
            String::from_utf8_lossy(&reuse.stderr)
        );
        write_control_info(&paths.info, &info).expect("restore matching starttime");
    }

    // --- missing token ---
    fs::remove_file(&paths.token).expect("remove token");
    let no_token = run_libra_command(&["code", "--control", "stdio"], main);
    assert!(!no_token.status.success());
    assert!(
        String::from_utf8_lossy(&no_token.stderr).contains("CONTROL_TOKEN_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&no_token.stderr)
    );
    write_control_token(&paths.token, "discovered-token\n");

    // --- info perms too open (Unix) ---
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.info, fs::Permissions::from_mode(0o644)).expect("widen info");
        let wide_info = run_libra_command(&["code", "--control", "stdio"], main);
        assert!(!wide_info.status.success());
        assert!(
            String::from_utf8_lossy(&wide_info.stderr).contains("CONTROL_INFO_PERMS"),
            "stderr={}",
            String::from_utf8_lossy(&wide_info.stderr)
        );
        write_control_info(&paths.info, &info).expect("restore 0600 info");
    }

    // --- token perms too open (Unix) ---
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.token, fs::Permissions::from_mode(0o644)).expect("widen");
        let wide = run_libra_command(&["code", "--control", "stdio"], main);
        assert!(!wide.status.success());
        assert!(
            String::from_utf8_lossy(&wide.stderr).contains("CONTROL_TOKEN_PERMS"),
            "stderr={}",
            String::from_utf8_lossy(&wide.stderr)
        );
        write_control_token(&paths.token, "discovered-token\n");
    }

    // --- scope / ownership mismatch (foreign worktree id) ---
    let mut foreign = info.clone();
    foreign.worktree_id = Some("foreign-wt".to_string());
    write_control_info(&paths.info, &foreign).expect("foreign scope");
    let scope = run_libra_command(&["code", "--control", "stdio"], main);
    assert!(!scope.status.success());
    assert!(
        String::from_utf8_lossy(&scope.stderr).contains("CONTROL_SCOPE_CONFLICT"),
        "stderr={}",
        String::from_utf8_lossy(&scope.stderr)
    );
    write_control_info(&paths.info, &info).expect("restore scope");

    // --- override precedence: explicit URL+token ignore planted baseUrl ---
    // Mock returns CONTROLLER_CONFLICT on attach to pin JSON-RPC error contract.
    async fn conflict_attach() -> impl IntoResponse {
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": {
                    "code": "CONTROLLER_CONFLICT",
                    "message": "another controller already holds the lease"
                }
            })),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("addr");
    let app = Router::new().route("/api/code/controller/attach", post(conflict_attach));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let override_token = main.join("override-token");
    write_control_token(&override_token, "override-token-value\n");
    let override_url = format!("http://{addr}");
    let attach_req = concat!(
        r#"{"jsonrpc":"2.0","method":"controller.attach","params":{"clientId":"w4-10","kind":"automation"},"id":1}"#,
        "\n",
    );
    let override_out = run_libra_command_with_stdin(
        &[
            "code",
            "--control",
            "stdio",
            "--control-url",
            &override_url,
            "--control-token-file",
            override_token.to_str().expect("utf8"),
        ],
        main,
        attach_req,
    );
    assert!(
        override_out.status.success(),
        "explicit overrides must reach the mock; stderr={}",
        String::from_utf8_lossy(&override_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&override_out.stdout);
    assert!(
        stdout.contains("CONTROLLER_CONFLICT") && stdout.contains("-32000"),
        "lease conflict must surface as JSON-RPC -32000 + CONTROLLER_CONFLICT; got={stdout}"
    );

    // --- default discovery uses planted control.json URL ---
    let mut live = info.clone();
    live.base_url = override_url.clone();
    write_control_info(&paths.info, &live).expect("point discovery at mock");
    write_control_token(&paths.token, "discovered-token\n");
    let discovered =
        run_libra_command_with_stdin(&["code", "--control", "stdio"], main, attach_req);
    assert!(
        discovered.status.success(),
        "default discovery must attach; stderr={}",
        String::from_utf8_lossy(&discovered.stderr)
    );
    let discovered_stdout = String::from_utf8_lossy(&discovered.stdout);
    assert!(
        discovered_stdout.contains("CONTROLLER_CONFLICT"),
        "discovered endpoint must hit mock attach; got={discovered_stdout}"
    );

    // --- custom --control-info-file reuses launch token path; override token when needed ---
    let alt_dir = main.join("alt-session");
    fs::create_dir_all(&alt_dir).expect("alt dir");
    let alt_info = alt_dir.join("control.json");
    write_control_info(&alt_info, &live).expect("alt info");
    // Default token still at worktree paths.token (producer/consumer pairing).
    write_control_token(&paths.token, "discovered-token\n");
    let alt = run_libra_command_with_stdin(
        &[
            "code",
            "--control",
            "stdio",
            "--control-info-file",
            alt_info.to_str().expect("utf8"),
        ],
        main,
        attach_req,
    );
    assert!(
        alt.status.success(),
        "--control-info-file must reuse default token path; stderr={}",
        String::from_utf8_lossy(&alt.stderr)
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_control_stdio_client_round_trips_attach_submit_detach() {
    #[derive(Default)]
    struct MockState {
        calls: Mutex<Vec<Value>>,
    }

    async fn attach(
        State(state): State<Arc<MockState>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.calls.lock().await.push(json!({
            "path": "attach",
            "token": headers.get("x-libra-control-token").and_then(|v| v.to_str().ok()),
            "body": body,
        }));
        Json(json!({
            "controllerToken": "lease-token",
            "leaseExpiresAt": "2026-04-30T00:00:00Z",
            "controller": { "kind": "automation", "canWrite": true, "loopbackOnly": true }
        }))
    }

    async fn messages(
        State(state): State<Arc<MockState>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.calls.lock().await.push(json!({
            "path": "messages",
            "token": headers.get("x-libra-control-token").and_then(|v| v.to_str().ok()),
            "controller": headers.get("x-code-controller-token").and_then(|v| v.to_str().ok()),
            "body": body,
        }));
        Json(json!({ "accepted": true }))
    }

    async fn detach(
        State(state): State<Arc<MockState>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.calls.lock().await.push(json!({
            "path": "detach",
            "token": headers.get("x-libra-control-token").and_then(|v| v.to_str().ok()),
            "controller": headers.get("x-code-controller-token").and_then(|v| v.to_str().ok()),
            "body": body,
        }));
        Json(json!({ "detached": true }))
    }

    let state = Arc::new(MockState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock control server");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let app = Router::new()
        .route("/api/code/controller/attach", post(attach))
        .route("/api/code/messages", post(messages))
        .route("/api/code/controller/detach", post(detach))
        .route("/api/code/session", get(|| async { Json(json!({})) }))
        .with_state(state.clone());
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let dir = tempdir().expect("tempdir");
    let token_path: PathBuf = dir.path().join("control.token");
    write_control_token(&token_path, "process-token\n");

    let url = format!("http://{addr}");
    let requests = concat!(
        r#"{"jsonrpc":"2.0","method":"controller.attach","params":{"clientId":"w4-02","kind":"automation"},"id":1}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"message.submit","params":{"text":"hello","controllerToken":"lease-token"},"id":2}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"controller.detach","params":{"clientId":"w4-02","controllerToken":"lease-token"},"id":3}"#,
        "\n",
    );

    let output = run_libra_command_with_stdin(
        &[
            "code",
            "--control",
            "stdio",
            "--control-url",
            &url,
            "--control-token-file",
            token_path.to_str().expect("utf8 token path"),
        ],
        dir.path(),
        requests,
    );
    assert!(
        output.status.success(),
        "canonical --control stdio must round-trip; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"result\"") || stdout.contains("controllerToken"),
        "JSON-RPC responses must appear on stdout; got={stdout}"
    );

    let calls = state.calls.lock().await.clone();
    assert!(
        calls.len() >= 3,
        "expected attach/messages/detach; got {calls:?}"
    );
    assert_eq!(calls[0]["path"], "attach");
    assert_eq!(calls[0]["token"], "process-token");
    assert_eq!(calls[1]["path"], "messages");
    assert_eq!(calls[1]["controller"], "lease-token");
    assert_eq!(calls[2]["path"], "detach");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}
