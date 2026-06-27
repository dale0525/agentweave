use anyhow::Context;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Component;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub source: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SkillCatalog {
    root: Option<PathBuf>,
    summaries: Vec<SkillSummary>,
}

#[derive(Debug, Deserialize)]
struct SkillFrontMatter {
    name: String,
    description: String,
    #[serde(default)]
    aliases: Vec<String>,
}

impl SkillCatalog {
    pub fn empty() -> Self {
        Self {
            root: None,
            summaries: Vec::new(),
        }
    }

    pub async fn load_development(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
        let mut summaries = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let skill_path = path.join("SKILL.md");
            if skill_path.is_file() {
                summaries.push(read_skill_summary(&canonical_root, &skill_path).await?);
            }
        }

        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        validate_unique_skill_names(&summaries)?;
        Ok(Self {
            root: Some(canonical_root),
            summaries,
        })
    }

    pub async fn load_packaged(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve packaged skill root {}", root.display()))?;
        let bytes = tokio::fs::read(root.join("skill-bundle.json"))
            .await
            .with_context(|| {
                format!("failed to read packaged skill index in {}", root.display())
            })?;
        let index: SkillBundleIndex = serde_json::from_slice(&bytes)?;
        let mut summaries = Vec::new();

        for entry in index.skills {
            if !entry.include_instructions {
                continue;
            }

            let skill_root = resolve_safe_catalog_path(root, &canonical_root, &entry.path).await?;
            let skill_path = skill_root.join("SKILL.md");
            if skill_path.is_file() {
                summaries.push(read_skill_summary(&canonical_root, &skill_path).await?);
            }
        }

        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        validate_unique_skill_names(&summaries)?;
        Ok(Self {
            root: Some(canonical_root),
            summaries,
        })
    }

    pub fn summaries(&self) -> &[SkillSummary] {
        &self.summaries
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
}

#[derive(Debug, Deserialize)]
struct SkillBundleIndex {
    skills: Vec<SkillBundleEntry>,
}

#[derive(Debug, Deserialize)]
struct SkillBundleEntry {
    path: PathBuf,
    #[serde(default)]
    include_instructions: bool,
}

fn parse_skill_front_matter(content: &str) -> anyhow::Result<SkillFrontMatter> {
    let Some(rest) = content.strip_prefix("---\n") else {
        anyhow::bail!("SKILL.md must start with front matter");
    };
    let Some((front_matter, _body)) = rest.split_once("\n---") else {
        anyhow::bail!("SKILL.md front matter must be closed");
    };

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut aliases = Vec::new();
    let mut in_aliases = false;

    for raw_line in front_matter.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        if let Some(value) = line.strip_prefix("name:") {
            name = Some(unquote_scalar(value.trim()));
            in_aliases = false;
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(unquote_scalar(value.trim()));
            in_aliases = false;
        } else if line.trim() == "aliases:" {
            in_aliases = true;
        } else if in_aliases {
            let trimmed = line.trim_start();
            let Some(value) = trimmed.strip_prefix("- ") else {
                anyhow::bail!("unsupported aliases entry in SKILL.md front matter");
            };
            aliases.push(unquote_scalar(value.trim()));
        }
    }

    let name = name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md front matter name must not be empty"))?;
    let description = description
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md front matter description must not be empty"))?;

    Ok(SkillFrontMatter {
        name,
        description,
        aliases,
    })
}

fn unquote_scalar(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|inner| inner.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_string()
}

async fn read_skill_summary(root: &Path, skill_path: &Path) -> anyhow::Result<SkillSummary> {
    let canonical_path = tokio::fs::canonicalize(skill_path)
        .await
        .with_context(|| format!("failed to resolve {}", skill_path.display()))?;
    if !canonical_path.starts_with(root) {
        anyhow::bail!("unsafe skill instruction path: {}", skill_path.display());
    }

    let content = tokio::fs::read_to_string(&canonical_path)
        .await
        .with_context(|| format!("failed to read {}", canonical_path.display()))?;
    let front_matter = parse_skill_front_matter(&content)?;
    let source = canonical_path
        .strip_prefix(root)
        .with_context(|| format!("make {} relative to root", canonical_path.display()))?
        .to_path_buf();

    Ok(SkillSummary {
        name: front_matter.name,
        description: front_matter.description,
        aliases: front_matter.aliases,
        source,
    })
}

fn validate_unique_skill_names(summaries: &[SkillSummary]) -> anyhow::Result<()> {
    let mut names = HashSet::new();
    for summary in summaries {
        if !names.insert(summary.name.as_str()) {
            anyhow::bail!("duplicate instruction skill name: {}", summary.name);
        }
    }

    Ok(())
}

async fn resolve_safe_catalog_path(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> anyhow::Result<PathBuf> {
    if !is_safe_catalog_path(path) {
        anyhow::bail!("unsafe packaged skill path: {}", path.display());
    }

    let candidate = root.join(path);
    let canonical_candidate = tokio::fs::canonicalize(&candidate)
        .await
        .with_context(|| format!("failed to resolve packaged skill path {}", path.display()))?;

    if !canonical_candidate.starts_with(canonical_root) {
        anyhow::bail!("unsafe packaged skill path: {}", path.display());
    }

    Ok(canonical_candidate)
}

fn is_safe_catalog_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    #[test]
    fn parses_skill_front_matter_with_aliases() {
        let front_matter = parse_skill_front_matter(
            r#"---
name: planning
description: Write implementation plans.
aliases:
  - plan-writer
  - planner
---

# Planning
"#,
        )
        .unwrap();

        assert_eq!(front_matter.name, "planning");
        assert_eq!(front_matter.description, "Write implementation plans.");
        assert_eq!(front_matter.aliases, vec!["plan-writer", "planner"]);
    }

    #[test]
    fn rejects_skill_without_front_matter() {
        let error = parse_skill_front_matter("# Missing metadata").unwrap_err();

        assert!(error.to_string().contains("front matter"));
    }

    #[test]
    fn rejects_skill_with_empty_description() {
        let error = parse_skill_front_matter(
            r#"---
name: empty-description
description: ""
---

# Empty
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("description"));
    }

    #[tokio::test]
    async fn development_catalog_discovers_instruction_only_skill() {
        let root = unique_test_dir("development-catalog");
        write_skill_md(
            &root,
            "planning",
            r#"---
name: planning
description: Write plans.
---

# Planning
"#,
        )
        .await;

        let catalog = SkillCatalog::load_development(&root).await.unwrap();

        assert_eq!(catalog.summaries()[0].name, "planning");
        assert_eq!(catalog.summaries()[0].description, "Write plans.");
        assert_eq!(
            catalog.summaries()[0].source,
            PathBuf::from("planning/SKILL.md")
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn packaged_catalog_includes_only_opted_in_skill_instructions() {
        let root = unique_test_dir("packaged-catalog");
        write_skill_md(
            &root,
            "included",
            r#"---
name: included
description: Included instructions.
---

# Included
"#,
        )
        .await;
        write_skill_md(
            &root,
            "excluded",
            r#"---
name: excluded
description: Excluded instructions.
---

# Excluded
"#,
        )
        .await;
        fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "included", "include_instructions": true },
                    { "path": "excluded" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let catalog = SkillCatalog::load_packaged(&root).await.unwrap();
        let names: Vec<_> = catalog
            .summaries()
            .iter()
            .map(|summary| summary.name.as_str())
            .collect();

        assert_eq!(names, vec!["included"]);
        remove_test_dir(root).await;
    }

    async fn write_skill_md(root: &Path, folder: &str, content: &str) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(skill_dir.join("SKILL.md"), content)
            .await
            .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "general-agent-skill-catalog-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            fs::remove_dir_all(path).await.unwrap();
        }
    }
}
