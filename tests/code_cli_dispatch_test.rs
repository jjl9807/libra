//! Wave 2 / PR 2 — `libra code` CLI dispatch L1 tests.
//!
//! Per `docs/development/commands/_general.md` §5.1, Wave 2's CLI surface must
//! cover mode selection, mutual exclusion, and parser smoke without
//! ever spawning the binary. We assert directly against
//! `clap::Parser::try_parse_from` so a bad flag combination fails
//! at parse time, before the runtime starts.
//!
//! What this file covers (P0 set):
//!
//! * `--web --stdio` mutual exclusion error.
//! * `--web` (alias for `--web-only`) parses to the same flag.
//! * `--mcp-port 0` is accepted (kernel-assigned port, used by the
//!   PTY harness).
//! * `--port 0` likewise.
//! * `--env-file` is parsed into the right field.
//! * `--repo`, `--cwd`, `--resume` pass through as `Some(...)`.
//! * `--browser-control loopback` together with `--stdio` is
//!   rejected (clap conflicts_with).
//!
//! What this file does NOT cover (deferred per the plan):
//!
//! * Provider boot smoke (Wave 10 / PR 10).
//! * `--plan-mode` default per provider — already covered by
//!   `effective_plan_mode_*` tests inside `src/command/code.rs`.
//! * TUI / MCP / Codex runtime — Waves 9/13.

use std::path::PathBuf;

use clap::Parser;
use libra::command::code::{CodeArgs, CodeContext, CodeNetworkAccess, CodeProvider, ControlMode};

/// Helper: parse `argv0 + args` with a fixed binary name. Strip the
/// `--web`/`--stdio` from the spelling caller passed since clap
/// expects the binary name as `argv[0]`.
fn parse(args: &[&str]) -> Result<CodeArgs, clap::Error> {
    let mut full: Vec<String> = vec!["code".to_string()];
    for arg in args {
        full.push((*arg).to_string());
    }
    CodeArgs::try_parse_from(full)
}

#[test]
fn web_only_and_stdio_are_mutually_exclusive() {
    let error = parse(&["--web-only", "--stdio"]).expect_err("clap must reject the combination");
    let rendered = error.to_string();
    // clap formats this as "argument '--stdio' cannot be used with '--web-only'".
    // We assert on both flag names instead of the exact phrasing so a
    // future clap upgrade doesn't break the test.
    assert!(
        rendered.contains("--stdio") && rendered.contains("--web-only"),
        "expected mutual-exclusion error to mention both flags; got: {rendered}",
    );
}

#[test]
fn web_alias_resolves_to_web_only() {
    let parsed = parse(&["--web"]).expect("--web is a documented alias");
    assert!(
        parsed.web_only,
        "--web must set web_only=true (alias for --web-only)",
    );
    assert!(!parsed.stdio, "--web must NOT enable stdio mode");
}

#[test]
fn web_and_stdio_are_mutually_exclusive_via_alias() {
    // Same conflict but exercised through the `--web` alias.
    let error = parse(&["--web", "--stdio"]).expect_err("alias must inherit conflicts_with");
    let rendered = error.to_string();
    assert!(
        rendered.contains("--stdio"),
        "expected --stdio in error: {rendered}"
    );
}

#[test]
fn mcp_port_zero_is_accepted() {
    let parsed = parse(&["--mcp-port", "0"]).expect("--mcp-port 0 is the kernel-pick sentinel");
    assert_eq!(parsed.mcp_port, 0);
}

#[test]
fn web_port_zero_is_accepted() {
    let parsed = parse(&["--port", "0"]).expect("--port 0 is the kernel-pick sentinel");
    assert_eq!(parsed.port, 0);
}

#[test]
fn env_file_parses_into_pathbuf() {
    let parsed = parse(&["--env-file", "/tmp/.env.test"]).expect(".env paths are valid input");
    assert_eq!(parsed.env_file, Some(PathBuf::from("/tmp/.env.test")));
}

#[test]
fn repo_and_cwd_and_resume_are_optional() {
    let bare = parse(&[]).expect("CodeArgs has no required positional args");
    assert!(bare.repo.is_none());
    assert!(bare.cwd.is_none());
    assert!(bare.resume.is_none());

    let with_paths = parse(&[
        "--repo",
        "/tmp/some-repo",
        "--cwd",
        "/tmp/some-cwd",
        "--resume",
        "thread-2026-05-10-001",
    ])
    .expect("--repo / --cwd / --resume are optional but well-typed");
    assert_eq!(with_paths.repo, Some(PathBuf::from("/tmp/some-repo")));
    assert_eq!(with_paths.cwd, Some(PathBuf::from("/tmp/some-cwd")));
    assert_eq!(with_paths.resume.as_deref(), Some("thread-2026-05-10-001"));
}

#[test]
fn browser_control_loopback_conflicts_with_stdio() {
    // `--browser-control loopback` is incompatible with `--stdio`
    // because the stdio MCP server has no HTTP surface for a
    // browser to attach to. clap's conflicts_with should reject.
    let error = parse(&["--browser-control", "loopback", "--stdio"])
        .expect_err("--browser-control + --stdio must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("--browser-control") && rendered.contains("--stdio"),
        "expected conflict error to mention both flags; got: {rendered}",
    );
}

#[test]
fn web_only_with_non_gemini_provider_parses() {
    // C2 (GAP-1): `--web-only --provider <non-gemini>` must parse cleanly at the
    // CLI layer; the previous web-only rejection lived in `validate_mode_args`,
    // not the parser, and is now relaxed (verified in code.rs unit tests).
    for provider in [
        "codex",
        "openai",
        "anthropic",
        "deepseek",
        "kimi",
        "zhipu",
        "ollama",
    ] {
        let parsed = parse(&["--web-only", "--provider", provider])
            .unwrap_or_else(|e| panic!("--web-only --provider {provider} must parse: {e}"));
        assert!(parsed.web_only);
        assert_ne!(parsed.provider, CodeProvider::Gemini);
    }
}

#[test]
fn web_only_with_provider_tuning_flags_parse() {
    // C2 (GAP-3): the provider-tuning flags the headless runtime consumes must
    // reach `CodeArgs` under `--web-only`.
    let parsed = parse(&[
        "--web-only",
        "--provider",
        "ollama",
        "--model",
        "llama3",
        "--api-base",
        "http://127.0.0.1:11434/v1",
        "--temperature",
        "0.2",
        "--ollama-thinking",
        "high",
    ])
    .expect("--web-only provider-tuning flags must parse");
    assert!(parsed.web_only);
    assert_eq!(parsed.provider, CodeProvider::Ollama);
    assert_eq!(parsed.model.as_deref(), Some("llama3"));
    assert_eq!(
        parsed.api_base.as_deref(),
        Some("http://127.0.0.1:11434/v1")
    );
    assert_eq!(parsed.temperature, Some(0.2));
    assert!(parsed.ollama_thinking.is_some());
}

#[test]
fn web_only_env_file_context_and_approval_flags_parse() {
    // W3-13: public TUI flags that feed headless bootstrap must parse under
    // `--web-only` (mode validation is covered in `code.rs` unit tests).
    let parsed = parse(&[
        "--web-only",
        "--env-file",
        "/tmp/.env.web-test",
        "--context",
        "dev",
        "--approval-policy",
        "on-request",
        "--approval-ttl",
        "42",
    ])
    .expect("--web-only must parse env-file/context/approval flags");
    assert!(parsed.web_only);
    assert_eq!(
        parsed.env_file.as_deref(),
        Some(std::path::Path::new("/tmp/.env.web-test"))
    );
    assert_eq!(parsed.context, Some(CodeContext::Dev));
    assert_eq!(parsed.approval_ttl, Some(42));
}

#[test]
fn defaults_are_observe_control_and_deny_network() {
    let bare = parse(&[]).expect("CodeArgs has no required args");
    // Spot-check that the documented defaults from publish.md /
    // docs/commands/code.md actually flow through.
    // ControlMode::Observe is the safe default (no automation
    // writes); CodeNetworkAccess::Deny is the safe default for
    // shell tools.
    //
    // Codex pass-1 P3: assert via PartialEq on the enum directly
    // instead of `format!("{:?}")` substring matching, which
    // would pass on accidental Debug-impl substring overlap.
    assert_eq!(
        bare.control,
        ControlMode::Observe,
        "control default must be ControlMode::Observe",
    );
    assert_eq!(
        bare.network_access,
        CodeNetworkAccess::Deny,
        "network_access default must be CodeNetworkAccess::Deny",
    );
}

#[test]
fn control_stdio_mode_parses_url_and_token_file() {
    let parsed = parse(&[
        "--control",
        "stdio",
        "--control-url",
        "http://127.0.0.1:3000",
        "--control-token-file",
        "/tmp/control.token",
    ])
    .expect("--control stdio must parse with explicit URL/token");
    assert_eq!(parsed.control, ControlMode::Stdio);
    assert_eq!(parsed.control_url.as_deref(), Some("http://127.0.0.1:3000"));
    assert_eq!(
        parsed.control_token_file.as_deref(),
        Some(std::path::Path::new("/tmp/control.token"))
    );
    assert!(!parsed.stdio, "--control stdio must not set MCP --stdio");
}

#[test]
fn control_stdio_mode_keeps_observe_write_parseable() {
    assert_eq!(
        parse(&["--control", "observe"]).expect("observe").control,
        ControlMode::Observe
    );
    assert_eq!(
        parse(&["--control", "write"]).expect("write").control,
        ControlMode::Write
    );
}

/// W3-11: occupied listen port fail-closes with an actionable `--port` hint
/// and never auto-increments to another free port.
#[tokio::test]
async fn default_port_conflict_fails_fast() {
    use libra::internal::ai::web::{WebServerOptions, describe_web_bind_error, start};

    let holder = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve a local port");
    let port = holder.local_addr().expect("local addr").port();
    let temp = tempfile::tempdir().expect("tempdir");

    let start_result = start(
        "127.0.0.1",
        port,
        temp.path().to_path_buf(),
        WebServerOptions::default(),
    )
    .await;
    let Err(err) = start_result else {
        panic!("second bind of the same port must fail closed");
    };

    let message = describe_web_bind_error("127.0.0.1", port, &err);
    assert!(
        message.contains("--port"),
        "operator message must mention --port; got: {message}"
    );
    assert!(
        message.contains("does not auto-scan") || message.contains("already in use"),
        "operator message must state fail-fast / no auto-scan; got: {message}"
    );

    // Holding `holder` until here proves we did not silently bind another port.
    drop(holder);
}

/// W4-01: default `libra code` (no `--web-only`) prints a Web URL, stays
/// resident without a TTY, and exits cleanly on SIGTERM (ports released).
#[cfg(unix)]
#[tokio::test]
async fn default_web_no_tty_and_sigterm_clean_shutdown() {
    use std::{
        io::{BufRead, BufReader, Read},
        net::TcpListener,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let repo_path = temp_dir.path();
    let home_dir = repo_path.join(".home");
    let config_home = home_dir.join(".config");
    std::fs::create_dir_all(&config_home).expect("isolated HOME");

    // Use the binary cargo already built for this test target — never nest
    // `cargo build` under `cargo test` (target-dir lock deadlock).
    let libra_bin = env!("CARGO_BIN_EXE_libra");

    let status = Command::new(libra_bin)
        .args(["init"])
        .current_dir(repo_path)
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home_dir)
        .status()
        .expect("libra init");
    assert!(status.success(), "libra init failed");

    // Let the child bind ephemeral ports (`--port 0` / `--mcp-port 0`) and
    // discover the URL from stdout. Pre-bind+drop races with parallel tests.
    let child = Command::new(libra_bin)
        .args(["code", "--port", "0", "--mcp-port", "0"])
        .current_dir(repo_path)
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home_dir)
        .env("GEMINI_API_KEY", "test-gemini-api-key")
        .env("LIBRA_TEST", "1") // skip best-effort browser open
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start default libra code");

    struct KillChildOnDrop(Option<std::process::Child>);
    impl Drop for KillChildOnDrop {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let mut child_guard = KillChildOnDrop(Some(child));

    let stdout = child_guard
        .0
        .as_mut()
        .expect("child")
        .stdout
        .take()
        .expect("stdout pipe");
    // Drain stdout on a dedicated thread for the child's whole lifetime.
    // Stopping early and dropping the pipe can SIGPIPE the child on the next
    // println! (bootstrap token / MCP URL), which then fails the health probe.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut captured = String::new();
        let mut line = String::new();
        let mut notified = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    captured.push_str(&line);
                    if !notified
                        && let Some(rest) =
                            line.trim().strip_prefix("Libra Code server running at ")
                    {
                        notified = true;
                        let _ = tx.send(rest.trim_end_matches('/').to_string());
                    }
                }
                Err(_) => break,
            }
        }
        let _ = done_tx.send(captured);
    });
    let printed_url = match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(url) => url,
        Err(_) => {
            let mut failed = child_guard.0.take().expect("child");
            let _ = failed.kill();
            let _ = failed.wait();
            let mut err = String::new();
            if let Some(mut stderr) = failed.stderr.take() {
                let _ = stderr.read_to_string(&mut err);
            }
            let captured = done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_default();
            panic!("timed out waiting for default Web bind URL; stdout={captured}; stderr={err}");
        }
    };
    let web_base = printed_url;
    assert!(
        web_base.starts_with("http://127.0.0.1:") || web_base.starts_with("http://[::1]:"),
        "default Web must bind loopback; got {web_base}"
    );
    assert!(
        web_base.contains("?bt="),
        "default Web open URL must embed browser bootstrap token; got {web_base}"
    );
    let web_origin = web_base
        .split_once('?')
        .map(|(origin, _)| origin)
        .unwrap_or(web_base.as_str());
    let web_port: u16 = web_origin
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .unwrap_or_else(|_| panic!("parse port from {web_base}"));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .unwrap();
    let health_url = format!("{web_origin}/api/health");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if Instant::now() > deadline {
            let mut failed = child_guard.0.take().expect("child");
            let _ = failed.kill();
            let _ = failed.wait();
            let mut err = String::new();
            if let Some(mut stderr) = failed.stderr.take() {
                let _ = stderr.read_to_string(&mut err);
            }
            panic!("default Web UI did not become healthy at {health_url}; stderr={err}");
        }
        if let Ok(resp) = client.get(&health_url).send().await
            && resp.status().is_success()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let pid = child_guard.0.as_ref().expect("child").id();
    let kill_status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(kill_status.success());

    let mut child = child_guard.0.take().expect("child");
    let exit = child
        .wait()
        .expect("wait for default Web process after SIGTERM");
    assert!(
        exit.success(),
        "SIGTERM must shut down cleanly (exit 0); status={exit:?}"
    );
    let captured_stdout = done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| String::new());

    assert!(
        captured_stdout.contains("Libra Code server running")
            || captured_stdout.contains(&web_base),
        "default Web launch must print the Code UI URL on stdout; got:\n{captured_stdout}"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    let rebind_web = TcpListener::bind(format!("127.0.0.1:{web_port}"));
    assert!(
        rebind_web.is_ok(),
        "web port must be released after SIGTERM"
    );
}

#[test]
fn mcp_stdio_deprecation_warning_pins_legacy_boundary() {
    use libra::command::code::MCP_STDIO_DEPRECATION_WARNING;

    assert!(
        MCP_STDIO_DEPRECATION_WARNING.contains("deprecated MCP-only legacy"),
        "W4-03 must mark --stdio as MCP-only legacy: {MCP_STDIO_DEPRECATION_WARNING}"
    );
    assert!(
        MCP_STDIO_DEPRECATION_WARNING.contains("not live turn control"),
        "W4-03 must exclude turn control: {MCP_STDIO_DEPRECATION_WARNING}"
    );
    assert!(
        MCP_STDIO_DEPRECATION_WARNING.contains("libra code --control stdio"),
        "W4-03 must point automation at --control stdio: {MCP_STDIO_DEPRECATION_WARNING}"
    );
    assert!(
        MCP_STDIO_DEPRECATION_WARNING.contains("libra mcp --stdio"),
        "W4-03 must point to future libra mcp --stdio (DEFER-02): {MCP_STDIO_DEPRECATION_WARNING}"
    );
}

#[test]
fn code_help_documents_mcp_stdio_legacy_and_control_client() {
    use clap::CommandFactory;
    use libra::command::code::{CODE_EXAMPLES, CodeArgs};

    let help = CodeArgs::command().render_long_help().to_string();
    assert!(
        help.contains("Deprecated MCP-only legacy") || help.contains("deprecated MCP-only legacy"),
        "clap --stdio help must document MCP-only legacy; got:\n{help}"
    );
    assert!(
        help.contains("libra mcp --stdio") || CODE_EXAMPLES.contains("libra mcp --stdio"),
        "help/examples must mention future libra mcp --stdio"
    );
    assert!(
        CODE_EXAMPLES.contains("--control stdio"),
        "EXAMPLES must keep canonical automation client"
    );
    assert!(
        CODE_EXAMPLES.contains("Deprecated MCP-only legacy"),
        "EXAMPLES must mark --stdio as deprecated MCP-only legacy"
    );
}
