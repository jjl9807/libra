//! §765 install-path assertion for `libra agent enable` (AG-19).
//!
//! Drives the built binary end-to-end (harness mirrors
//! `tests/agent_lifecycle_event_test.rs`) and pins the installer contract:
//! every hook command the enable path writes embeds the **canonical
//! absolute path** of the running Libra binary — never a bare `libra`
//! PATH lookup — for both the OpenCode plugin file
//! (`src/internal/ai/hooks/providers/opencode/settings.rs`) and the Codex
//! `$CODEX_HOME/hooks.json` + `config.toml` trust entries
//! (`src/internal/ai/hooks/providers/codex/settings.rs`), plus the Codex
//! trust-gap stderr banner and marker-respecting disable semantics.

#![cfg(unix)]

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

/// The eleven Codex events the installer forwards, with their CLI verbs
/// (pins `CODEX_HOOK_FORWARD_MAP` in codex/settings.rs).
const CODEX_FORWARDED_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("PreToolUse", "tool-use"),
    ("PostToolUse", "tool-use"),
    ("PermissionRequest", "permission-request"),
    ("PreCompact", "compaction"),
    ("PostCompact", "compaction"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
    ("SubagentStart", "subagent-start"),
    ("SubagentStop", "subagent-end"),
];

/// First line of every Libra-managed OpenCode plugin file (pins
/// `LIBRA_MANAGED_MARKER` in opencode/settings.rs).
const OPENCODE_MANAGED_MARKER: &str =
    "// libra-managed: do not edit — installed by libra agent enable (AG-19)";

/// One isolated libra repository plus a fake `$HOME`.
struct HookRepo {
    _tempdir: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl HookRepo {
    fn init() -> Self {
        let tempdir = tempfile::tempdir().expect("create tempdir");
        let home = tempdir.path().join("home");
        let repo = tempdir.path().join("repo");
        std::fs::create_dir_all(&home).expect("create fake home");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        let this = Self {
            _tempdir: tempdir,
            repo,
            home,
        };
        let out = this.run(&["init"], None, &[]);
        assert!(
            out.status.success(),
            "libra init failed: {}",
            describe(&out)
        );
        this
    }

    fn run(&self, args: &[&str], stdin: Option<&str>, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
        cmd.args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("LIBRA_TEST_HOME", &self.home)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn libra binary");
        if let Some(payload) = stdin {
            child
                .stdin
                .take()
                .expect("stdin piped")
                .write_all(payload.as_bytes())
                .expect("write hook envelope to stdin");
        }
        child.wait_with_output().expect("wait for libra binary")
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

/// Canonical absolute path of the binary under test — the exact string the
/// installers must embed (`resolve_hook_binary_path` canonicalizes
/// `current_exe`, and the CLI passes `current_exe` through).
fn canonical_binary_path() -> String {
    std::fs::canonicalize(env!("CARGO_BIN_EXE_libra"))
        .expect("canonicalize the built libra binary")
        .display()
        .to_string()
}

/// `agent enable --agent opencode` writes the Libra-managed plugin at
/// `<repo>/.opencode/plugin/libra-hooks.js` with the marker first line and
/// the canonical absolute binary path baked into `LIBRA_COMMAND`; the hook
/// invocation interpolates that constant (never a bare `libra`). Disable
/// removes the managed file, and a user-owned file at the same path
/// survives a second disable.
#[test]
fn opencode_enable_writes_canonical_binary_path() {
    let repo = HookRepo::init();
    let canonical = canonical_binary_path();
    let plugin_path = repo
        .repo
        .join(".opencode")
        .join("plugin")
        .join("libra-hooks.js");

    let out = repo.run(
        &["agent", "enable", "--agent", "opencode", "--json"],
        None,
        &[],
    );
    assert!(out.status.success(), "enable opencode: {}", describe(&out));

    let content = std::fs::read_to_string(&plugin_path)
        .unwrap_or_else(|err| panic!("plugin missing at {}: {err}", plugin_path.display()));
    assert!(
        content.starts_with(OPENCODE_MANAGED_MARKER),
        "plugin must start with the Libra-managed marker line:\n{content}"
    );

    // The command constant must carry the canonical absolute binary path…
    let command_line = content
        .lines()
        .find(|line| line.starts_with("const LIBRA_COMMAND = "))
        .unwrap_or_else(|| panic!("no LIBRA_COMMAND line in plugin:\n{content}"));
    assert!(
        command_line.contains(&canonical),
        "LIBRA_COMMAND must embed the canonical binary path '{canonical}': {command_line}"
    );
    // …and never a bare PATH-dependent `libra` token.
    for bare in [
        r#"const LIBRA_COMMAND = "libra";"#,
        r#"const LIBRA_COMMAND = "'libra'";"#,
    ] {
        assert_ne!(
            command_line, bare,
            "hook command must not be a bare PATH lookup"
        );
    }
    // The actual invocation goes through the pinned constant (raw shell
    // interpolation), so no handler ever spells out a bare `libra ` call.
    assert!(
        content.contains("${{ raw: LIBRA_COMMAND }} agent hooks opencode ${verb}"),
        "the forward invocation must interpolate LIBRA_COMMAND:\n{content}"
    );

    // Disable removes the managed file.
    let out = repo.run(&["agent", "disable", "--agent", "opencode"], None, &[]);
    assert!(out.status.success(), "disable opencode: {}", describe(&out));
    assert!(
        !plugin_path.exists(),
        "disable must remove the Libra-managed plugin file"
    );

    // A user-owned file (no marker) at the same path survives another
    // disable byte-for-byte.
    let user_content = "export const MyPlugin = async () => ({});\n";
    std::fs::write(&plugin_path, user_content).expect("seed user plugin file");
    let out = repo.run(&["agent", "disable", "--agent", "opencode"], None, &[]);
    assert!(
        out.status.success(),
        "second disable must stay a safe no-op: {}",
        describe(&out)
    );
    assert_eq!(
        std::fs::read_to_string(&plugin_path).expect("read user plugin back"),
        user_content,
        "a user file without the marker must survive disable"
    );
}

/// `agent enable --agent codex` writes `$CODEX_HOME/hooks.json` handlers
/// that all start with the canonical binary path + ` hooks codex ` (the
/// stable installed surface is the top-level `libra hooks codex <verb>`
/// entry, which routes to AgentTraces; `agent hooks codex` is the legacy
/// hidden spelling), plus one `[hooks.state."…"]` trust section per
/// forwarded event (6) with a `sha256:` trusted_hash. A trusted install
/// ingests session-start with no trust-gap banner; tampering one hash
/// makes the banner name exactly one gap; disable removes the Libra
/// hooks.json entries and config.toml state sections.
#[test]
fn codex_enable_writes_canonical_binary_path_and_trust_entries() {
    let repo = HookRepo::init();
    let canonical = canonical_binary_path();

    let codex_home = repo.home.join(".codex");
    let codex_home_str = codex_home.display().to_string();
    let codex_env: &[(&str, &str)] = &[("CODEX_HOME", codex_home_str.as_str())];
    let hooks_path = codex_home.join("hooks.json");
    let config_path = codex_home.join("config.toml");

    let out = repo.run(&["agent", "enable", "--agent", "codex"], None, codex_env);
    assert!(out.status.success(), "enable codex: {}", describe(&out));

    // hooks.json: every forwarded event carries exactly the canonical
    // command, and every Libra-managed handler starts with the canonical
    // absolute path (no bare `libra`).
    let hooks_json: Value = serde_json::from_str(
        &std::fs::read_to_string(&hooks_path)
            .unwrap_or_else(|err| panic!("hooks.json missing at {}: {err}", hooks_path.display())),
    )
    .expect("hooks.json parses as JSON");
    let events = hooks_json["hooks"]
        .as_object()
        .unwrap_or_else(|| panic!("hooks.json has no hooks object: {hooks_json}"));
    for (event, verb) in CODEX_FORWARDED_EVENTS {
        let expected = format!("{canonical} hooks codex {verb}");
        let found = events
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|groups| {
                groups.iter().any(|group| {
                    group["hooks"].as_array().is_some_and(|handlers| {
                        handlers.iter().any(|handler| {
                            handler["type"] == json!("command")
                                && handler["command"] == json!(expected.clone())
                        })
                    })
                })
            });
        assert!(
            found,
            "event '{event}' must carry the canonical command '{expected}': {hooks_json}"
        );
    }
    let managed_prefix = format!("{canonical} hooks codex ");
    for (event, groups) in events {
        for group in groups.as_array().into_iter().flatten() {
            for handler in group["hooks"].as_array().into_iter().flatten() {
                let command = handler["command"].as_str().unwrap_or_default();
                if command.contains(" hooks codex ") {
                    assert!(
                        command.starts_with(&managed_prefix),
                        "Libra handler under '{event}' must start with the canonical \
                         binary path: {command}"
                    );
                }
            }
        }
    }

    // config.toml: one `[hooks.state."…"]` section per forwarded event,
    // each with a sha256 trusted_hash.
    let config = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|err| panic!("config.toml missing at {}: {err}", config_path.display()));
    assert_eq!(
        config.matches("[hooks.state.\"").count(),
        CODEX_FORWARDED_EVENTS.len(),
        "one trust section per forwarded event expected:\n{config}"
    );
    assert_eq!(
        config.matches("trusted_hash = \"sha256:").count(),
        CODEX_FORWARDED_EVENTS.len(),
        "every trust section must carry a sha256 trusted_hash:\n{config}"
    );

    // A trusted install ingests session-start silently (gaps == 0 → no
    // banner on stderr). Use the exact surface the installed commands
    // invoke: the top-level `libra hooks codex <verb>` entry.
    let envelope = json!({
        "hook_event_name": "SessionStart",
        "session_id": "sess-codex-trust",
        "cwd": repo.repo.to_string_lossy(),
    })
    .to_string();
    let out = repo.run(
        &["hooks", "codex", "session-start"],
        Some(&envelope),
        codex_env,
    );
    assert!(
        out.status.success(),
        "trusted session-start ingest: {}",
        describe(&out)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("not locally approved"),
        "zero trust gaps must render no banner: {}",
        describe(&out)
    );

    // Tamper exactly one trusted_hash → the next session-start still exits
    // 0 (banner only, never blocking) and names exactly one gap.
    let needle = "trusted_hash = \"";
    let start = config.find(needle).expect("a trusted_hash line to tamper") + needle.len();
    let end = start + config[start..].find('"').expect("closing quote");
    let tampered = format!("{}sha256:0000{}", &config[..start], &config[end..]);
    std::fs::write(&config_path, &tampered).expect("write tampered config.toml");

    let out = repo.run(
        &["hooks", "codex", "session-start"],
        Some(&envelope),
        codex_env,
    );
    assert!(
        out.status.success(),
        "the trust-gap banner must never block the hook: {}",
        describe(&out)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("1 Libra-managed Codex hook(s) are not locally approved"),
        "banner must name exactly one gap: {}",
        describe(&out)
    );

    // Repair trust before disabling: `hooks_are_installed` fails closed on
    // trust gaps ("installed but untrusted" reads as not installed), so a
    // disable issued against the tampered config would skip the uninstall.
    let out = repo.run(&["agent", "enable", "--agent", "codex"], None, codex_env);
    assert!(out.status.success(), "re-enable codex: {}", describe(&out));

    // Disable removes the Libra hooks.json entries and every trust section.
    let out = repo.run(&["agent", "disable", "--agent", "codex"], None, codex_env);
    assert!(out.status.success(), "disable codex: {}", describe(&out));
    let hooks_after = std::fs::read_to_string(&hooks_path).expect("hooks.json is never deleted");
    assert!(
        !hooks_after.contains(" hooks codex "),
        "disable must remove every Libra-managed handler:\n{hooks_after}"
    );
    let config_after = std::fs::read_to_string(&config_path).expect("config.toml still readable");
    assert!(
        !config_after.contains("[hooks.state.\""),
        "disable must remove every Libra trust section:\n{config_after}"
    );
    assert!(
        !config_after.contains("libra-managed codex hook trust entry"),
        "disable must remove the Libra marker comments:\n{config_after}"
    );
}
