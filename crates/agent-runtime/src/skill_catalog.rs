use serde::Deserialize;
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

    pub fn summaries(&self) -> &[SkillSummary] {
        &self.summaries
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
