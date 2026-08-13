//! W4-11: `[approval]` section of `.libra/config.toml` via the unified resolver.

use std::{fs, path::Path, time::Duration};

use serde::Deserialize;

use super::{ApprovalCachePolicy, DEFAULT_APPROVAL_TTL};
use crate::internal::ai::sources::security::{
    format_security_parse_error, request_scope_for_workdir, resolve_security_file,
    toml_error_location,
};

#[derive(Debug, Deserialize)]
struct ApprovalProjectConfig {
    approval: Option<ApprovalSectionConfig>,
}

#[derive(Debug, Deserialize)]
struct ApprovalSectionConfig {
    ttl_seconds: Option<u64>,
    #[serde(default)]
    protected_branches: Option<Vec<String>>,
    /// `None` = field omitted; `Some([])` = explicit empty allowlist.
    #[serde(default)]
    allowed_network_domains: Option<Vec<String>>,
    #[serde(default)]
    no_cache_unknown_network: bool,
}

#[derive(Debug, Default, Clone)]
struct ApprovalLayer {
    ttl: Option<Duration>,
    protected_branches: Option<Vec<String>>,
    allowed_network_domains: Option<Vec<String>>,
    no_cache_unknown_network: bool,
}

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ApprovalProjectRuntimeConfig {
    pub ttl: Option<Duration>,
    pub cache_policy: ApprovalCachePolicy,
}

/// Load `[approval]` from `config.toml` with RequestScope + tighten-only overlay.
///
/// Missing file → defaults. Unreadable or malformed repository/overlay TOML
/// fails closed. Overlay may only add protected branches, intersect network
/// allowlists (including an explicit empty overlay list), raise
/// `no_cache_unknown_network`, and shorten TTL.
pub fn load_approval_project_config(
    working_dir: &Path,
) -> Result<ApprovalProjectRuntimeConfig, String> {
    if let Some(request) = request_scope_for_workdir(working_dir)? {
        let resolved = resolve_security_file(&request, "config.toml")?;
        let repository = parse_approval_bytes(
            &resolved.repository_bytes,
            "repository",
            &resolved.provenance.repository_path,
        )?;
        let Some(overlay_bytes) = resolved.overlay_bytes.as_deref() else {
            return Ok(runtime_from_layer(repository));
        };
        let overlay_path = resolved
            .provenance
            .overlay_path
            .as_deref()
            .unwrap_or_else(|| Path::new("<overlay>"));
        let overlay = parse_approval_bytes(overlay_bytes, "overlay", overlay_path)?;
        return Ok(runtime_from_layer(merge_approval_tighten_only(
            repository, overlay,
        )));
    }

    let path = working_dir.join(".libra").join("config.toml");
    match fs::read_to_string(&path) {
        Ok(contents) => {
            parse_approval_bytes(contents.as_bytes(), "project", &path).map(runtime_from_layer)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ApprovalProjectRuntimeConfig::default())
        }
        Err(error) => Err(format!(
            "failed to read approval config `{}`: {error}",
            path.display()
        )),
    }
}

fn parse_approval_bytes(bytes: &[u8], layer: &str, path: &Path) -> Result<ApprovalLayer, String> {
    if bytes.is_empty() {
        return Ok(ApprovalLayer::default());
    }
    let contents = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "failed to parse approval config ({layer} at `{}`): {error}",
            path.display()
        )
    })?;
    let config: ApprovalProjectConfig = toml::from_str(contents).map_err(|error| {
        format_security_parse_error(
            "approval config",
            layer,
            path,
            toml_error_location(contents, &error),
        )
    })?;
    Ok(layer_from_section(config.approval, path))
}

fn layer_from_section(approval: Option<ApprovalSectionConfig>, path: &Path) -> ApprovalLayer {
    let Some(approval) = approval else {
        return ApprovalLayer::default();
    };
    let ttl = approval.ttl_seconds.and_then(|ttl_seconds| {
        if ttl_seconds == 0 {
            tracing::warn!(
                target: "libra::ai::sandbox::approval_config",
                path = %path.display(),
                "ignoring approval ttl_seconds=0"
            );
            None
        } else {
            Some(Duration::from_secs(ttl_seconds))
        }
    });
    ApprovalLayer {
        ttl,
        protected_branches: approval.protected_branches,
        allowed_network_domains: approval.allowed_network_domains,
        no_cache_unknown_network: approval.no_cache_unknown_network,
    }
}

fn effective_repository_defaults(mut repository: ApprovalLayer) -> ApprovalLayer {
    if repository.ttl.is_none() {
        repository.ttl = Some(DEFAULT_APPROVAL_TTL);
    }
    if repository.protected_branches.is_none() {
        repository.protected_branches = Some(ApprovalCachePolicy::default().protected_branches);
    }
    if repository.no_cache_unknown_network && repository.allowed_network_domains.is_none() {
        // Omit + no_cache_unknown_network is deny-all for cached network hosts.
        repository.allowed_network_domains = Some(Vec::new());
    }
    repository
}

fn merge_approval_tighten_only(repository: ApprovalLayer, overlay: ApprovalLayer) -> ApprovalLayer {
    let repository = effective_repository_defaults(repository);
    let ttl = match (repository.ttl, overlay.ttl) {
        (Some(repo), Some(over)) => Some(repo.min(over)),
        (Some(repo), None) => Some(repo),
        (None, Some(over)) => Some(over),
        (None, None) => None,
    };

    let protected_branches = match (repository.protected_branches, overlay.protected_branches) {
        (None, None) => None,
        (Some(repo), None) => Some(repo),
        (None, Some(over)) => Some(over),
        (Some(mut repo), Some(over)) => {
            for branch in over {
                if !repo.iter().any(|existing| existing == &branch) {
                    repo.push(branch);
                }
            }
            Some(repo)
        }
    };

    let allowed_network_domains = match (
        repository.allowed_network_domains,
        overlay.allowed_network_domains,
    ) {
        (None, None) => None,
        (Some(repo), None) => Some(repo),
        (None, Some(over)) => Some(over),
        (Some(repo), Some(over)) => Some(
            repo.into_iter()
                .filter(|domain| over.iter().any(|overlay_domain| overlay_domain == domain))
                .collect(),
        ),
    };

    ApprovalLayer {
        ttl,
        protected_branches,
        allowed_network_domains,
        no_cache_unknown_network: repository.no_cache_unknown_network
            || overlay.no_cache_unknown_network,
    }
}

fn runtime_from_layer(layer: ApprovalLayer) -> ApprovalProjectRuntimeConfig {
    let default_cache_policy = ApprovalCachePolicy::default();
    ApprovalProjectRuntimeConfig {
        ttl: layer.ttl,
        cache_policy: ApprovalCachePolicy {
            protected_branches: layer
                .protected_branches
                .unwrap_or(default_cache_policy.protected_branches),
            allowed_network_domains: layer.allowed_network_domains.unwrap_or_default(),
            no_cache_unknown_network: layer.no_cache_unknown_network,
            approved_ruleset: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_with_domains(domains: Option<Vec<&str>>) -> ApprovalLayer {
        ApprovalLayer {
            allowed_network_domains: domains.map(|d| d.into_iter().map(str::to_string).collect()),
            ..ApprovalLayer::default()
        }
    }

    #[test]
    fn explicit_empty_overlay_allowlist_intersects_to_empty() {
        let repository = layer_with_domains(Some(vec!["github.com"]));
        let overlay = layer_with_domains(Some(Vec::new()));
        let merged = runtime_from_layer(merge_approval_tighten_only(repository, overlay));
        assert!(
            merged.cache_policy.allowed_network_domains.is_empty(),
            "explicit empty overlay must revoke repository allowlist entries"
        );
    }

    #[test]
    fn overlay_cannot_add_domains_when_repository_denies_unknown_network() {
        let repository = ApprovalLayer {
            allowed_network_domains: None,
            no_cache_unknown_network: true,
            ..ApprovalLayer::default()
        };
        let overlay = layer_with_domains(Some(vec!["github.com"]));
        let merged = runtime_from_layer(merge_approval_tighten_only(repository, overlay));
        assert!(
            merged.cache_policy.allowed_network_domains.is_empty(),
            "overlay must not re-enable cached network hosts when repository deny-all is set"
        );
        assert!(merged.cache_policy.no_cache_unknown_network);
    }

    #[test]
    fn omitted_overlay_allowlist_keeps_repository_domains() {
        let repository = layer_with_domains(Some(vec!["github.com"]));
        let overlay = layer_with_domains(None);
        let merged = runtime_from_layer(merge_approval_tighten_only(repository, overlay));
        assert_eq!(
            merged.cache_policy.allowed_network_domains,
            vec!["github.com".to_string()]
        );
    }

    #[test]
    fn overlay_cannot_lengthen_default_ttl_when_repository_omits_it() {
        let overlay = ApprovalLayer {
            ttl: Some(Duration::from_secs(3600)),
            ..ApprovalLayer::default()
        };
        let merged = runtime_from_layer(merge_approval_tighten_only(
            ApprovalLayer::default(),
            overlay,
        ));
        assert_eq!(merged.ttl, Some(DEFAULT_APPROVAL_TTL));
    }

    #[test]
    fn overlay_can_shorten_default_ttl_when_repository_omits_it() {
        let overlay = ApprovalLayer {
            ttl: Some(Duration::from_secs(60)),
            ..ApprovalLayer::default()
        };
        let merged = runtime_from_layer(merge_approval_tighten_only(
            ApprovalLayer::default(),
            overlay,
        ));
        assert_eq!(merged.ttl, Some(Duration::from_secs(60)));
    }

    #[test]
    fn overlay_cannot_replace_default_protected_branches() {
        let overlay = ApprovalLayer {
            protected_branches: Some(vec!["develop".to_string()]),
            ..ApprovalLayer::default()
        };
        let merged = runtime_from_layer(merge_approval_tighten_only(
            ApprovalLayer::default(),
            overlay,
        ));
        let branches = &merged.cache_policy.protected_branches;
        assert!(branches.iter().any(|b| b == "main"));
        assert!(branches.iter().any(|b| b == "master"));
        assert!(branches.iter().any(|b| b == "develop"));
    }

    #[test]
    fn parse_distinguishes_omitted_and_explicit_empty_allowlist() {
        let omitted = parse_approval_bytes(
            b"[approval]\nno_cache_unknown_network = true\n",
            "test",
            Path::new("omitted.toml"),
        )
        .expect("omitted");
        assert!(omitted.allowed_network_domains.is_none());

        let empty = parse_approval_bytes(
            b"[approval]\nallowed_network_domains = []\nno_cache_unknown_network = true\n",
            "test",
            Path::new("empty.toml"),
        )
        .expect("empty");
        assert_eq!(empty.allowed_network_domains.as_deref(), Some(&[][..]));
    }

    #[test]
    fn malformed_approval_toml_does_not_echo_file_contents() {
        let secret = "sk-leaked-token-value";
        let error = parse_approval_bytes(
            format!("[approval]\nttl_seconds = \"{secret}\"\n").as_bytes(),
            "repository",
            Path::new(".libra/config.toml"),
        )
        .expect_err("malformed");
        assert!(
            error.contains("failed to parse approval config")
                && error.contains("repository")
                && error.contains("config.toml"),
            "got {error}"
        );
        assert!(
            !error.contains(secret),
            "parse diagnostic must not echo file contents: {error}"
        );
    }
}
