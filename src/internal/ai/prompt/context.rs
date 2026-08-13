//! Context modes for mode-specific system prompt injection.

use std::{fmt, path::Path};

use super::loader::project_security_dir_paths;

/// Operating context that adjusts the AI agent's behavior and priorities.
///
/// Contexts are injected as an additional section in the system prompt,
/// modifying how the agent approaches tasks without changing the base rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMode {
    /// Active development: write code first, explain after.
    Dev,
    /// Code review: thorough analysis, severity-based reporting.
    Review,
    /// Research: explore and understand before acting.
    Research,
    /// Custom context loaded from a file or string.
    Custom(String),
}

impl ContextMode {
    /// Returns the embedded default content for built-in contexts.
    pub fn embedded_content(&self) -> &str {
        match self {
            ContextMode::Dev => include_str!("embedded/contexts/dev.md"),
            ContextMode::Review => include_str!("embedded/contexts/review.md"),
            ContextMode::Research => include_str!("embedded/contexts/research.md"),
            ContextMode::Custom(content) => content.as_str(),
        }
    }

    /// Returns the filename for this context (used for filesystem overrides).
    pub fn filename(&self) -> Option<&str> {
        match self {
            ContextMode::Dev => Some("dev"),
            ContextMode::Review => Some("review"),
            ContextMode::Research => Some("research"),
            ContextMode::Custom(_) => None,
        }
    }

    /// Load context content, checking filesystem overrides first.
    ///
    /// Override paths checked:
    /// 1. Repository `.libra/contexts/{name}.md` (W4-11; overlay adds only when
    ///    the repository file is absent)
    /// 2. `~/.config/libra/contexts/{name}.md`
    /// 3. Embedded default
    pub fn load_content(&self, working_dir: &Path) -> Result<String, String> {
        if let Some(filename) = self.filename() {
            let md_name = format!("{}.md", filename);
            let (repository, overlay) = project_security_dir_paths(working_dir, "contexts")?;
            if !repository.as_os_str().is_empty() {
                match super::loader::probe_security_file(&repository.join(&md_name))? {
                    Some(content) if !content.trim().is_empty() => return Ok(content),
                    Some(_) => {
                        // Blank repository file blocks overlay replacement.
                    }
                    None => {
                        if let Some(overlay) = overlay
                            && let Some(content) =
                                read_nonempty_security_context(&overlay.join(&md_name))?
                        {
                            return Ok(content);
                        }
                    }
                }
            }

            if let Some(config_dir) = dirs::config_dir() {
                let user_path = config_dir.join("libra").join("contexts").join(&md_name);
                if let Ok(content) = std::fs::read_to_string(&user_path)
                    && !content.trim().is_empty()
                {
                    return Ok(content);
                }
            }
        }

        Ok(self.embedded_content().to_string())
    }
}

fn read_nonempty_security_context(path: &Path) -> Result<Option<String>, String> {
    Ok(super::loader::probe_security_file(path)?.filter(|content| !content.trim().is_empty()))
}

impl fmt::Display for ContextMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextMode::Dev => write!(f, "dev"),
            ContextMode::Review => write!(f, "review"),
            ContextMode::Research => write!(f, "research"),
            ContextMode::Custom(_) => write!(f, "custom"),
        }
    }
}

impl std::str::FromStr for ContextMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dev" | "development" => Ok(ContextMode::Dev),
            "review" | "code-review" => Ok(ContextMode::Review),
            "research" | "explore" => Ok(ContextMode::Research),
            other => Err(format!(
                "unknown context '{}', expected: dev, review, research",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_embedded_content_nonempty() {
        assert!(!ContextMode::Dev.embedded_content().is_empty());
        assert!(!ContextMode::Review.embedded_content().is_empty());
        assert!(!ContextMode::Research.embedded_content().is_empty());
    }

    #[test]
    fn test_custom_context() {
        let ctx = ContextMode::Custom("My custom context".to_string());
        assert_eq!(ctx.embedded_content(), "My custom context");
        assert!(ctx.filename().is_none());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", ContextMode::Dev), "dev");
        assert_eq!(format!("{}", ContextMode::Review), "review");
        assert_eq!(format!("{}", ContextMode::Research), "research");
        assert_eq!(format!("{}", ContextMode::Custom("x".into())), "custom");
    }

    #[test]
    fn test_from_str() {
        assert_eq!("dev".parse::<ContextMode>().unwrap(), ContextMode::Dev);
        assert_eq!(
            "review".parse::<ContextMode>().unwrap(),
            ContextMode::Review
        );
        assert_eq!(
            "research".parse::<ContextMode>().unwrap(),
            ContextMode::Research
        );
        assert_eq!(
            "development".parse::<ContextMode>().unwrap(),
            ContextMode::Dev
        );
        assert_eq!(
            "code-review".parse::<ContextMode>().unwrap(),
            ContextMode::Review
        );
        assert_eq!(
            "explore".parse::<ContextMode>().unwrap(),
            ContextMode::Research
        );
        assert!("unknown".parse::<ContextMode>().is_err());
    }

    #[test]
    fn test_load_content_uses_embedded_default() {
        let tmp = TempDir::new().unwrap();
        let content = ContextMode::Dev
            .load_content(tmp.path())
            .expect("embedded context");
        assert!(content.contains("Development Mode"));
    }

    #[test]
    fn test_load_content_project_override() {
        let tmp = TempDir::new().unwrap();
        let ctx_dir = tmp.path().join(".libra").join("contexts");
        std::fs::create_dir_all(&ctx_dir).unwrap();
        std::fs::write(ctx_dir.join("dev.md"), "Custom dev context").unwrap();

        let content = ContextMode::Dev
            .load_content(tmp.path())
            .expect("project context");
        assert_eq!(content, "Custom dev context");
    }

    #[test]
    fn test_load_content_custom_ignores_filesystem() {
        let tmp = TempDir::new().unwrap();
        let ctx = ContextMode::Custom("inline content".to_string());
        let content = ctx.load_content(tmp.path()).expect("custom context");
        assert_eq!(content, "inline content");
    }
}
