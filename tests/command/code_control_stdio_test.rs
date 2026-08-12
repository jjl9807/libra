//! W4-02: canonical `libra code --control stdio` JSON-RPC client.
//!
//! L1 — deterministic. Covers CLI conflict surface via the real binary and
//! attach/submit/detach through the shared
//! [`libra::command::code_control::run_control_stdio_client`] helper (same
//! transport as legacy `code-control --stdio`).

use std::{fs, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

use super::*;

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
fn code_control_stdio_rejects_missing_url_and_token() {
    let repo = tempdir().expect("tempdir");
    let output = run_libra_command(&["code", "--control", "stdio"], repo.path());
    assert!(
        !output.status.success(),
        "missing --control-url/--control-token-file must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--control-url") || stderr.contains("control-url"),
        "usage must mention --control-url; stderr={stderr}"
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
    fs::write(&token, "process-token\n").expect("write token");
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
    fs::write(&token_path, "process-token\n").expect("write token");

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
