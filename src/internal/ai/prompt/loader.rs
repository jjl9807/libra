//! Three-tier rule loader: repository (+ overlay add) > user-global > embedded defaults.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use tracing::debug;

use super::rules::{RuleCategory, RuleFile};
use crate::internal::ai::sources::security::{request_scope_for_workdir, resolve_security_dir};

/// Load a single rule, checking overrides in priority order:
///
/// 1. Repository `.libra/rules/{category}.md` (W4-11 resolver; overlay may only
///    add a file when the repository layer is absent)
/// 2. `~/.config/libra/rules/{category}.md` (user-global override)
/// 3. Embedded default compiled into the binary
///
/// Unreadable repository/overlay directories or files fail closed.
pub fn load_rule(category: RuleCategory, working_dir: &Path) -> Result<RuleFile, String> {
    let filename = format!("{}.md", category.filename());

    if let Some(content) = read_project_rule(working_dir, &filename)? {
        debug!(category = %category, "loaded project-local rule override");
        return Ok(RuleFile { category, content });
    }

    if let Some(config_dir) = dirs::config_dir() {
        let user_path = config_dir.join("libra").join("rules").join(&filename);
        if let Some(content) = read_non_empty_optional(&user_path) {
            debug!(category = %category, path = %user_path.display(), "loaded user-global rule override");
            return Ok(RuleFile { category, content });
        }
    }

    debug!(category = %category, "using embedded default rule");
    Ok(RuleFile {
        category,
        content: category.embedded_content().to_string(),
    })
}

/// Load all rules in prompt composition order.
pub fn load_all_rules(working_dir: &Path) -> Result<Vec<RuleFile>, String> {
    RuleCategory::all_in_order()
        .iter()
        .map(|&cat| load_rule(cat, working_dir))
        .collect()
}

pub(super) fn project_security_dir_paths(
    working_dir: &Path,
    location: &'static str,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    if let Some(request) = request_scope_for_workdir(working_dir)? {
        let resolved = resolve_security_dir(&request, location)?;
        return Ok((resolved.repository_path, resolved.overlay_path));
    }
    let fallback = match location {
        "rules" => working_dir.join(".libra").join("rules"),
        "contexts" => working_dir.join(".libra").join("contexts"),
        other => working_dir.join(".libra").join(other),
    };
    Ok((fallback, None))
}

fn read_project_rule(working_dir: &Path, filename: &str) -> Result<Option<String>, String> {
    let (repository, overlay) = project_security_dir_paths(working_dir, "rules")?;
    if repository.as_os_str().is_empty() {
        return Ok(None);
    }
    match probe_security_file(&repository.join(filename))? {
        Some(content) if !content.trim().is_empty() => return Ok(Some(content)),
        Some(_) => {
            // Present but blank: keep embedded/user fallback. Overlay must not
            // replace a repository placeholder.
            return Ok(None);
        }
        None => {}
    }
    if let Some(overlay) = overlay {
        return read_nonempty_security_file(&overlay.join(filename));
    }
    Ok(None)
}

/// Missing → `None`; present (including blank) → `Some`; other IO errors fail closed.
///
/// `NotFound` is accepted only when `symlink_metadata` says the path itself
/// is absent. A dangling symlink (metadata exists, `read_to_string` NotFound)
/// fails closed so repository rules/contexts cannot silently drop.
pub(super) fn probe_security_file(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read security config `{}`: {error}",
                path.display()
            ));
        }
        Ok(_) => {}
    }
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) => Err(format!(
            "failed to read security config `{}`: {error}",
            path.display()
        )),
    }
}

fn read_nonempty_security_file(path: &Path) -> Result<Option<String>, String> {
    Ok(probe_security_file(path)?.filter(|content| !content.trim().is_empty()))
}

/// User-global / non-security optional read: missing or unreadable → skip.
fn read_non_empty_optional(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_load_rule_returns_embedded_default() {
        let tmp = TempDir::new().unwrap();
        let rule = load_rule(RuleCategory::Base, tmp.path()).expect("embedded rule");
        assert_eq!(rule.category, RuleCategory::Base);
        assert!(!rule.content.is_empty());
        assert!(rule.content.contains("{working_dir}"));
    }

    #[test]
    fn test_load_all_rules_returns_all_categories() {
        let tmp = TempDir::new().unwrap();
        let rules = load_all_rules(tmp.path()).expect("embedded rules");
        assert_eq!(rules.len(), RuleCategory::all_in_order().len());
        for (rule, &expected_cat) in rules.iter().zip(RuleCategory::all_in_order()) {
            assert_eq!(rule.category, expected_cat);
        }
    }

    #[test]
    fn test_project_local_override_takes_precedence() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("base.md"), "Custom base rule content").unwrap();

        let rule = load_rule(RuleCategory::Base, tmp.path()).expect("project rule");
        assert_eq!(rule.content, "Custom base rule content");
    }

    #[test]
    fn test_empty_override_falls_back_to_embedded() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("base.md"), "   \n  ").unwrap();

        let rule = load_rule(RuleCategory::Base, tmp.path()).expect("fallback");
        // Should fall back to embedded since override is whitespace-only
        assert!(rule.content.contains("{working_dir}"));
    }

    #[test]
    fn test_all_embedded_rules_load_without_panic() {
        let tmp = TempDir::new().unwrap();
        for &category in RuleCategory::all_in_order() {
            let rule = load_rule(category, tmp.path()).expect("embedded");
            assert!(
                !rule.content.is_empty(),
                "{:?} should have content",
                category
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_unreadable_project_rule_fails_closed() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        let rule_path = rules_dir.join("base.md");
        std::fs::write(&rule_path, "secret-project-rule").unwrap();
        std::fs::set_permissions(&rule_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let error = load_rule(RuleCategory::Base, tmp.path())
            .expect_err("unreadable project rule must not fall back to embedded");
        assert!(
            error.contains("failed to read") && error.contains("base.md"),
            "got {error}"
        );

        std::fs::set_permissions(&rule_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_dangling_symlink_project_rule_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::os::unix::fs::symlink("/no/such/libra-rule-target", rules_dir.join("base.md"))
            .unwrap();

        let error = load_rule(RuleCategory::Base, tmp.path())
            .expect_err("dangling symlink must not fall back to embedded");
        assert!(
            error.contains("failed to read") && error.contains("base.md"),
            "got {error}"
        );
    }
}
