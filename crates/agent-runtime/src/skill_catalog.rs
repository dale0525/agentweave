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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInstructionDocument {
    pub name: String,
    pub source: PathBuf,
    pub content: String,
    pub truncated: bool,
    pub read_bytes: usize,
    pub original_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct SkillCatalogEntry {
    pub summary: SkillSummary,
    pub document: SkillInstructionDocument,
}

#[derive(Clone, Debug)]
pub struct SkillCatalog {
    root: Option<PathBuf>,
    entries: Vec<SkillCatalogEntry>,
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
            entries: Vec::new(),
            summaries: Vec::new(),
        }
    }

    pub fn from_entries(mut entries: Vec<SkillCatalogEntry>) -> anyhow::Result<Self> {
        for entry in &entries {
            validate_catalog_entry(entry)?;
        }
        entries.sort_by(|left, right| left.summary.name.cmp(&right.summary.name));
        let summaries = entries
            .iter()
            .map(|entry| entry.summary.clone())
            .collect::<Vec<_>>();
        validate_unique_skill_names(&summaries)?;
        Ok(Self {
            root: None,
            entries,
            summaries,
        })
    }

    #[deprecated(
        note = "development/test compatibility only; production hosts must use SkillManager"
    )]
    pub async fn load_development(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
        let mut catalog_entries = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let skill_path = path.join("SKILL.md");
            if skill_path.is_file() {
                let mut catalog_entry = Self::read_package_entry(&path).await?;
                set_entry_source(
                    &mut catalog_entry,
                    PathBuf::from(entry.file_name()).join("SKILL.md"),
                );
                catalog_entries.push(catalog_entry);
            }
        }

        let mut catalog = Self::from_entries(catalog_entries)?;
        catalog.root = Some(canonical_root);
        Ok(catalog)
    }

    pub async fn read_development_skill_summary(
        root: impl AsRef<Path>,
        skill_path: impl AsRef<Path>,
    ) -> anyhow::Result<SkillSummary> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
        Ok(
            read_instruction_entry(&canonical_root, skill_path.as_ref(), None)
                .await?
                .summary,
        )
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
        let mut catalog_entries = Vec::new();

        for entry in index.skills {
            if !entry.include_instructions {
                continue;
            }

            let source = entry.path.join("SKILL.md");
            let skill_root = resolve_safe_catalog_path(root, &canonical_root, &entry.path).await?;
            let skill_path = skill_root.join("SKILL.md");
            if skill_path.is_file() {
                let mut catalog_entry = Self::read_package_entry(&skill_root).await?;
                set_entry_source(&mut catalog_entry, source);
                catalog_entries.push(catalog_entry);
            }
        }

        let mut catalog = Self::from_entries(catalog_entries)?;
        catalog.root = Some(canonical_root);
        Ok(catalog)
    }

    pub async fn read_package_entry(package_root: &Path) -> anyhow::Result<SkillCatalogEntry> {
        let root_metadata = tokio::fs::symlink_metadata(package_root)
            .await
            .with_context(|| {
                format!("failed to inspect package root {}", package_root.display())
            })?;
        if root_metadata.file_type().is_symlink() {
            anyhow::bail!(
                "skill package root must not be a symlink: {}",
                package_root.display()
            );
        }
        if !root_metadata.is_dir() {
            anyhow::bail!(
                "skill package root must be a directory: {}",
                package_root.display()
            );
        }
        let canonical_root = tokio::fs::canonicalize(package_root)
            .await
            .with_context(|| {
                format!("failed to resolve package root {}", package_root.display())
            })?;
        read_instruction_entry(
            &canonical_root,
            &package_root.join("SKILL.md"),
            Some(PathBuf::from("SKILL.md")),
        )
        .await
    }

    pub fn read_verified_package_entry(
        source: PathBuf,
        bytes: &[u8],
    ) -> anyhow::Result<SkillCatalogEntry> {
        let content = std::str::from_utf8(bytes)
            .context("verified skill instructions are not UTF-8")?
            .to_string();
        instruction_entry_from_content(content, source)
    }

    pub fn summaries(&self) -> &[SkillSummary] {
        &self.summaries
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn triggered_skill_names(&self, user_text: &str) -> Vec<String> {
        let tokens = trigger_tokens(user_text);
        let dollar_tokens: HashSet<_> = tokens
            .iter()
            .filter_map(|token| token.strip_prefix('$'))
            .collect();
        let plain_tokens: HashSet<_> = tokens
            .iter()
            .filter(|token| !token.starts_with('$'))
            .map(String::as_str)
            .collect();
        let mut triggered = HashSet::new();

        for summary in &self.summaries {
            if dollar_tokens.contains(summary.name.to_ascii_lowercase().as_str()) {
                triggered.insert(summary.name.clone());
            }
        }

        for token in plain_tokens {
            let matches: Vec<_> = self
                .summaries
                .iter()
                .filter(|summary| summary.matches_plain_token(token))
                .map(|summary| summary.name.clone())
                .collect();
            if matches.len() == 1 {
                triggered.insert(matches[0].clone());
            }
        }

        let mut names: Vec<_> = triggered.into_iter().collect();
        names.sort();
        names
    }

    pub async fn load_instruction_documents(
        &self,
        names: &[String],
        max_instruction_bytes: usize,
    ) -> anyhow::Result<Vec<SkillInstructionDocument>> {
        let mut documents = Vec::new();

        for name in names {
            let entry = self
                .entries
                .iter()
                .find(|entry| entry.summary.name == *name)
                .ok_or_else(|| anyhow::anyhow!("unknown instruction skill: {name}"))?;
            let mut document = entry.document.clone();
            if document.content.len() > max_instruction_bytes {
                let boundary = previous_char_boundary(&document.content, max_instruction_bytes);
                document.content.truncate(boundary);
                document.read_bytes = boundary;
                document.truncated = true;
            }
            documents.push(document);
        }

        Ok(documents)
    }
}

impl SkillSummary {
    fn matches_plain_token(&self, token: &str) -> bool {
        self.name.eq_ignore_ascii_case(token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(token))
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
    if value.starts_with('"')
        && value.ends_with('"')
        && let Ok(decoded) = serde_json::from_str::<String>(value)
    {
        return decoded;
    }
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

async fn read_instruction_entry(
    root: &Path,
    skill_path: &Path,
    source: Option<PathBuf>,
) -> anyhow::Result<SkillCatalogEntry> {
    let metadata = tokio::fs::symlink_metadata(skill_path)
        .await
        .with_context(|| format!("failed to inspect {}", skill_path.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "skill instruction path must not be a symlink: {}",
            skill_path.display()
        );
    }
    if !metadata.is_file() {
        anyhow::bail!(
            "skill instruction path must be a file: {}",
            skill_path.display()
        );
    }
    let canonical_path = tokio::fs::canonicalize(skill_path)
        .await
        .with_context(|| format!("failed to resolve {}", skill_path.display()))?;
    if !canonical_path.starts_with(root) {
        anyhow::bail!("unsafe skill instruction path: {}", skill_path.display());
    }

    let content = tokio::fs::read_to_string(&canonical_path)
        .await
        .with_context(|| format!("failed to read {}", canonical_path.display()))?;
    let source = match source {
        Some(source) => source,
        None => canonical_path
            .strip_prefix(root)
            .with_context(|| format!("make {} relative to root", canonical_path.display()))?
            .to_path_buf(),
    };
    instruction_entry_from_content(content, source)
}

fn instruction_entry_from_content(
    content: String,
    source: PathBuf,
) -> anyhow::Result<SkillCatalogEntry> {
    let front_matter = parse_skill_front_matter(&content)?;
    let original_bytes = content.len();
    let name = front_matter.name;
    Ok(SkillCatalogEntry {
        summary: SkillSummary {
            name: name.clone(),
            description: front_matter.description,
            aliases: front_matter.aliases,
            source: source.clone(),
        },
        document: SkillInstructionDocument {
            name,
            source,
            content,
            truncated: false,
            read_bytes: original_bytes,
            original_bytes,
        },
    })
}

fn set_entry_source(entry: &mut SkillCatalogEntry, source: PathBuf) {
    entry.summary.source = source.clone();
    entry.document.source = source;
}

fn previous_char_boundary(content: &str, limit: usize) -> usize {
    let mut boundary = limit.min(content.len());
    while !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn validate_catalog_entry(entry: &SkillCatalogEntry) -> anyhow::Result<()> {
    if entry.summary.name != entry.document.name {
        anyhow::bail!(
            "instruction entry summary name does not match document name: {} != {}",
            entry.summary.name,
            entry.document.name
        );
    }
    if entry.summary.source != entry.document.source {
        anyhow::bail!(
            "instruction entry summary source does not match document source for {}: {} != {}",
            entry.summary.name,
            entry.summary.source.display(),
            entry.document.source.display()
        );
    }
    if entry.document.truncated {
        anyhow::bail!(
            "instruction entry document must not be truncated: {}",
            entry.summary.name
        );
    }
    let content_bytes = entry.document.content.len();
    if entry.document.read_bytes != content_bytes {
        anyhow::bail!(
            "instruction entry read_bytes must equal content length for {}: {} != {}",
            entry.summary.name,
            entry.document.read_bytes,
            content_bytes
        );
    }
    if entry.document.original_bytes != content_bytes {
        anyhow::bail!(
            "instruction entry original_bytes must equal content length for {}: {} != {}",
            entry.summary.name,
            entry.document.original_bytes,
            content_bytes
        );
    }
    Ok(())
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

fn trigger_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == '$' && current.is_empty() {
            current.push(ch);
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
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
    async fn project_catalog_loads_release_ready_project_instruction_skills() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");

        let catalog = SkillCatalog::load_development(skills_root).await.unwrap();
        let names: Vec<_> = catalog
            .summaries()
            .iter()
            .map(|summary| summary.name.as_str())
            .collect();

        assert!(names.contains(&"filesystem"));
        assert!(names.contains(&"skill-creator"));
        assert!(!names.contains(&"test-driven-development"));
    }

    #[tokio::test]
    async fn read_development_skill_summary_validates_one_skill_file() {
        let root = unique_test_dir("single-instruction-package");
        write_skill_md(
            &root,
            "planning",
            "---\nname: planning\ndescription: Plan work.\n---\n\n# Planning",
        )
        .await;

        let summary = SkillCatalog::read_development_skill_summary(
            &root,
            root.join("planning").join("SKILL.md"),
        )
        .await
        .unwrap();

        assert_eq!(summary.name, "planning");
        assert_eq!(summary.source, PathBuf::from("planning/SKILL.md"));
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

    #[tokio::test]
    async fn loads_full_instruction_for_triggered_skill_with_truncation_metadata() {
        let root = unique_test_dir("load-full-instruction");
        write_skill_md(
            &root,
            "planning",
            r#"---
name: planning
description: Write plans.
---

# Planning
Use checklists.
"#,
        )
        .await;
        let catalog = SkillCatalog::load_development(&root).await.unwrap();

        let instructions = catalog
            .load_instruction_documents(&["planning".to_string()], 32)
            .await
            .unwrap();

        assert_eq!(instructions[0].name, "planning");
        assert_eq!(instructions[0].source, PathBuf::from("planning/SKILL.md"));
        assert!(instructions[0].content.contains("---"));
        assert!(instructions[0].truncated);
        remove_test_dir(root).await;
    }

    #[test]
    fn trigger_policy_matches_explicit_dollar_skill() {
        let catalog = SkillCatalog {
            root: None,
            entries: Vec::new(),
            summaries: vec![SkillSummary {
                name: "planning".into(),
                description: "Write plans.".into(),
                aliases: vec![],
                source: PathBuf::from("planning/SKILL.md"),
            }],
        };

        assert_eq!(
            catalog.triggered_skill_names("please use $planning"),
            vec!["planning".to_string()]
        );
    }

    #[test]
    fn trigger_policy_matches_unique_plain_text_name_or_alias() {
        let catalog = SkillCatalog {
            root: None,
            entries: Vec::new(),
            summaries: vec![
                SkillSummary {
                    name: "planning".into(),
                    description: "Write plans.".into(),
                    aliases: vec!["planner".into()],
                    source: PathBuf::from("planning/SKILL.md"),
                },
                SkillSummary {
                    name: "debugging".into(),
                    description: "Debug issues.".into(),
                    aliases: vec![],
                    source: PathBuf::from("debugging/SKILL.md"),
                },
            ],
        };

        assert_eq!(
            catalog.triggered_skill_names("planner should help"),
            vec!["planning".to_string()]
        );
    }

    #[test]
    fn trigger_policy_ignores_ambiguous_plain_text_mentions() {
        let catalog = SkillCatalog {
            root: None,
            entries: Vec::new(),
            summaries: vec![
                SkillSummary {
                    name: "plan".into(),
                    description: "Plan A.".into(),
                    aliases: vec!["shared".into()],
                    source: PathBuf::from("a/SKILL.md"),
                },
                SkillSummary {
                    name: "planning".into(),
                    description: "Plan B.".into(),
                    aliases: vec!["shared".into()],
                    source: PathBuf::from("b/SKILL.md"),
                },
            ],
        };

        assert!(catalog.triggered_skill_names("shared").is_empty());
    }

    #[tokio::test]
    async fn runtime_skill_and_instruction_skill_can_coexist() {
        let root = unique_test_dir("coexist");
        write_skill_md(
            &root,
            "coexist",
            "---\nname: coexist\ndescription: Both forms.\n---\n\n# Coexist",
        )
        .await;
        write_runtime_skill_json(&root, "coexist", "coexist_echo").await;

        let catalog = SkillCatalog::load_development(&root).await.unwrap();
        let registry = crate::skill::SkillRegistry::load_development(&root)
            .await
            .unwrap();

        assert!(
            catalog
                .summaries()
                .iter()
                .any(|summary| summary.name == "coexist")
        );
        assert!(
            registry
                .tools()
                .iter()
                .any(|tool| tool.name == "coexist_echo")
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn runtime_tool_without_skill_md_still_executes() {
        let root = unique_test_dir("runtime-only");
        write_runtime_skill_json(&root, "runtime-only", "runtime_only_echo").await;

        let catalog = SkillCatalog::load_development(&root).await.unwrap();
        let registry = crate::skill::SkillRegistry::load_development(&root)
            .await
            .unwrap();
        let result = registry
            .execute("runtime_only_echo", serde_json::json!({ "text": "ok" }))
            .await
            .unwrap();

        assert!(catalog.summaries().is_empty());
        assert_eq!(result["text"], "ok");
        remove_test_dir(root).await;
    }

    async fn write_skill_md(root: &Path, folder: &str, content: &str) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(skill_dir.join("SKILL.md"), content)
            .await
            .unwrap();
    }

    async fn write_runtime_skill_json(root: &Path, folder: &str, tool_name: &str) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": folder,
                "description": "Echo a text payload.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": tool_name,
                        "description": "Return the provided text.",
                        "input_schema": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            },
                            "required": ["text"]
                        }
                    }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();
        fs::write(
            skill_dir.join("index.js"),
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
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
