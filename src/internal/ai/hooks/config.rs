//! Hook configuration loading and per-tool matching.
//!
//! This module is the on-disk surface of the hook system. It deserialises the
//! `hooks.json` files described in [`super`] and exposes the merged set of hook
//! definitions that the runtime executes when lifecycle events fire.
//!
//! Repository + overlay tiers are merged tighten-only (W4-11): overlay cannot
//! drop or disable a repository `PreToolUse` hook. User-global hooks are then
//! concatenated. Missing files are a valid empty state; unreadable or
//! malformed repository/overlay JSON fails closed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::event::HookEvent;
use crate::internal::ai::sources::security::{
    format_security_parse_error, json_error_location, request_scope_for_workdir,
    resolve_security_file,
};

/// A single hook definition as read from `hooks.json`.
///
/// Each definition binds a [`HookEvent`] (lifecycle trigger) to a shell `command` and,
/// for tool-scoped events, a `matcher` that filters which tool invocations fire it.
/// Default values for `timeout_ms` and `enabled` keep older configs forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Which lifecycle event triggers this hook.
    pub event: HookEvent,
    /// Tool name pattern to match (plain string or pipe-separated alternatives).
    /// Empty or `"*"` matches all tools. Ignored for session events.
    #[serde(default)]
    pub matcher: String,
    /// Shell command to execute. Receives JSON on stdin.
    pub command: String,
    /// Human-readable description of what this hook does.
    #[serde(default)]
    pub description: String,
    /// Timeout in milliseconds. Defaults to 10_000 (10s).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Whether this hook is enabled. Defaults to true.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Default hook timeout in milliseconds. Chosen to be long enough for typical lint /
/// formatter invocations but short enough to keep a runaway script from stalling the
/// agent loop.
fn default_timeout() -> u64 {
    10_000
}

/// Hooks default to enabled when the `enabled` field is omitted from JSON.
fn default_enabled() -> bool {
    true
}

impl HookDefinition {
    /// Decide whether this hook should fire for a tool with the given name.
    ///
    /// Functional scope:
    /// - An empty matcher or `"*"` is treated as a wildcard and matches every tool.
    /// - Otherwise the matcher is split on `|` and trimmed, supporting compact
    ///   alternation like `"Edit|Write|apply_patch"`.
    ///
    /// Boundary conditions:
    /// - The match is exact; substring matches are intentionally rejected to avoid
    ///   accidentally enabling a hook for unrelated tools.
    /// - Whitespace inside each alternative is trimmed, but punctuation is not.
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        if self.matcher.is_empty() || self.matcher == "*" {
            return true;
        }
        // Support pipe-separated alternatives: "Edit|Write|apply_patch"
        self.matcher
            .split('|')
            .any(|pattern| pattern.trim() == tool_name)
    }

    fn pre_tool_use_key(&self) -> Option<(&str, &str)> {
        if self.event == HookEvent::PreToolUse {
            Some((self.matcher.as_str(), self.command.as_str()))
        } else {
            None
        }
    }
}

/// Root document persisted in `hooks.json`.
///
/// The file is intentionally a single object so that future fields (e.g. metadata,
/// schema version) can be added without breaking older parsers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// List of hook definitions.
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
}

/// Load hook configuration from repository (+ overlay) + user tiers.
///
/// Functional scope:
/// - Inside a Libra repository, reads via the W4-06 resolver (`hooks.json`
///   repository layer, then optional linked-worktree overlay).
/// - Outside a repository (tests / non-repo use), reads
///   `<working_dir>/.libra/hooks.json`.
/// - Then appends the user-global file at `<config_dir>/libra/hooks.json`.
///
/// Boundary conditions:
/// - Missing files are skipped — running without hooks is a valid state.
/// - Unreadable or malformed **repository/overlay** JSON fails closed with a
///   diagnostic that names the source layer and omits file contents.
/// - Malformed **user-global** JSON is logged at `warn` and ignored so a
///   broken personal config never blocks the agent.
/// - Overlay cannot delete or disable a repository `PreToolUse` hook.
/// - When `dirs::config_dir()` returns `None` only the project tiers load.
pub fn load_hook_config(working_dir: &Path) -> Result<HookConfig, String> {
    let mut all_hooks = load_project_hooks(working_dir)?;

    if let Some(config_dir) = dirs::config_dir() {
        let user_config = config_dir.join("libra").join("hooks.json");
        if let Some(config) = load_user_config_file(&user_config) {
            all_hooks.extend(config.hooks);
        }
    }

    Ok(HookConfig { hooks: all_hooks })
}

fn load_project_hooks(working_dir: &Path) -> Result<Vec<HookDefinition>, String> {
    if let Some(request) = request_scope_for_workdir(working_dir)? {
        let resolved = resolve_security_file(&request, "hooks.json")?;
        let repository = parse_hook_bytes(
            &resolved.repository_bytes,
            "repository",
            &resolved.provenance.repository_path,
        )?;
        let overlay = match resolved.overlay_bytes.as_deref() {
            Some(bytes) => {
                let overlay_path = resolved
                    .provenance
                    .overlay_path
                    .as_deref()
                    .unwrap_or_else(|| Path::new("<overlay>"));
                parse_hook_bytes(bytes, "overlay", overlay_path)?
            }
            None => HookConfig::default(),
        };
        return Ok(merge_hooks_tighten_only(repository.hooks, overlay.hooks));
    }

    let project_config = working_dir.join(".libra").join("hooks.json");
    Ok(parse_hook_file_optional(&project_config)?
        .unwrap_or_default()
        .hooks)
}

fn parse_hook_bytes(bytes: &[u8], layer: &str, path: &Path) -> Result<HookConfig, String> {
    if bytes.is_empty() {
        return Ok(HookConfig::default());
    }
    let content = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "failed to parse hook config ({layer} at `{}`): {error}",
            path.display()
        )
    })?;
    serde_json::from_str(content).map_err(|error| {
        format_security_parse_error("hook config", layer, path, json_error_location(&error))
    })
}

fn parse_hook_file_optional(path: &Path) -> Result<Option<HookConfig>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read hook config `{}`: {error}",
                path.display()
            ));
        }
    };
    serde_json::from_str(&content).map(Some).map_err(|error| {
        format_security_parse_error("hook config", "project", path, json_error_location(&error))
    })
}

fn merge_hooks_tighten_only(
    repository: Vec<HookDefinition>,
    overlay: Vec<HookDefinition>,
) -> Vec<HookDefinition> {
    let mut merged = Vec::with_capacity(repository.len() + overlay.len());
    merged.extend(repository);
    let repo_pretool: Vec<(String, String)> = merged
        .iter()
        .filter_map(|hook| {
            hook.pre_tool_use_key()
                .map(|(matcher, command)| (matcher.to_string(), command.to_string()))
        })
        .collect();
    for hook in overlay {
        if let Some((matcher, command)) = hook.pre_tool_use_key()
            && repo_pretool.iter().any(|(repo_matcher, repo_command)| {
                repo_matcher == matcher && repo_command == command
            })
        {
            // Overlay cannot replace or disable a repository PreToolUse Block.
            continue;
        }
        merged.push(hook);
    }
    merged
}

/// Try to read and parse a single user-global `hooks.json`.
///
/// Returns `None` when the file does not exist, cannot be read, or fails to parse.
/// Parse errors are surfaced via `tracing::warn` so operators can debug a broken file
/// without losing the rest of the agent session.
fn load_user_config_file(path: &Path) -> Option<HookConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content)
        .map_err(|e| {
            tracing::warn!("Failed to parse hook config {}: {}", path.display(), e);
            e
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario: a hook listing alternatives matches each named tool but not others.
    #[test]
    fn test_hook_definition_matches_tool() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "read_file|list_dir".to_string(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("read_file"));
        assert!(hook.matches_tool("list_dir"));
        assert!(!hook.matches_tool("apply_patch"));
    }

    // Scenario: `"*"` is the explicit wildcard that fires on every tool name.
    #[test]
    fn test_hook_definition_wildcard() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("read_file"));
        assert!(hook.matches_tool("anything"));
    }

    // Scenario: an omitted `matcher` field is treated identically to `"*"`.
    #[test]
    fn test_hook_definition_empty_matcher() {
        let hook = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: String::new(),
            command: "echo test".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };

        assert!(hook.matches_tool("anything"));
    }

    // Scenario: a fresh working directory with no hooks.json yields an empty config.
    #[test]
    fn test_load_hook_config_missing_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = load_hook_config(tmp.path()).expect("missing hooks is empty");
        assert!(config.hooks.is_empty());
    }

    // Scenario: project-local hooks.json is loaded when present in `.libra/`.
    #[test]
    fn test_load_hook_config_from_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let hook_dir = tmp.path().join(".libra");
        std::fs::create_dir_all(&hook_dir).unwrap();
        std::fs::write(
            hook_dir.join("hooks.json"),
            r#"{"hooks": [{"event": "pre_tool_use", "matcher": "shell", "command": "echo blocked"}]}"#,
        )
        .unwrap();

        let config = load_hook_config(tmp.path()).expect("project hooks");
        assert_eq!(config.hooks.len(), 1);
        assert_eq!(config.hooks[0].matcher, "shell");
    }

    // Scenario: malformed project hooks fail closed (do not silently drop PreToolUse).
    #[test]
    fn test_load_hook_config_malformed_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let hook_dir = tmp.path().join(".libra");
        std::fs::create_dir_all(&hook_dir).unwrap();
        std::fs::write(hook_dir.join("hooks.json"), "{not json").unwrap();

        let error = load_hook_config(tmp.path()).expect_err("malformed hooks fail closed");
        assert!(
            error.contains("parse") && error.contains("hooks.json"),
            "got {error}"
        );
    }

    // Scenario: repository PreToolUse enabled=false stays disabled after overlay merge.
    #[test]
    fn test_repository_disabled_pretool_stays_disabled() {
        let repo = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "shell".to_string(),
            command: "echo intentionally-off".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: false,
        };
        let overlay_reenable = HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "shell".to_string(),
            command: "echo intentionally-off".to_string(),
            description: String::new(),
            timeout_ms: 10_000,
            enabled: true,
        };
        let merged = merge_hooks_tighten_only(vec![repo], vec![overlay_reenable]);
        assert_eq!(merged.len(), 1);
        assert!(
            !merged[0].enabled,
            "overlay must not re-enable a repository PreToolUse that is disabled"
        );
    }

    // Scenario: full JSON round-trip with both explicit and default-filled fields.
    #[test]
    fn test_deserialize_hook_config() {
        let json = r#"{
            "hooks": [
                {
                    "event": "pre_tool_use",
                    "matcher": "shell",
                    "command": "node check.js",
                    "description": "Block dangerous shell commands",
                    "timeout_ms": 5000,
                    "enabled": true
                },
                {
                    "event": "post_tool_use",
                    "matcher": "apply_patch",
                    "command": "cargo fmt",
                    "description": "Format after edit"
                }
            ]
        }"#;

        let config: HookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.hooks.len(), 2);
        assert_eq!(config.hooks[0].event, HookEvent::PreToolUse);
        assert_eq!(config.hooks[0].timeout_ms, 5000);
        assert_eq!(config.hooks[1].event, HookEvent::PostToolUse);
        assert_eq!(config.hooks[1].timeout_ms, 10_000); // default
        assert!(config.hooks[1].enabled); // default
    }
}
