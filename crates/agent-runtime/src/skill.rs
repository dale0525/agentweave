use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub entry: SkillEntry,
    pub tools: Vec<SkillTool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub root: PathBuf,
    pub manifest: SkillManifest,
}

pub struct SkillRegistry {
    skills: Vec<InstalledSkill>,
}

impl SkillRegistry {
    pub async fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut skills = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let manifest_path = path.join("skill.json");
            if !manifest_path.is_file() {
                continue;
            }

            let bytes = tokio::fs::read(&manifest_path).await?;
            let manifest: SkillManifest = serde_json::from_slice(&bytes)?;
            skills.push(InstalledSkill {
                root: path,
                manifest,
            });
        }

        Ok(Self { skills })
    }

    pub fn tools(&self) -> Vec<SkillTool> {
        self.skills
            .iter()
            .flat_map(|skill| skill.manifest.tools.clone())
            .collect()
    }

    pub async fn execute(&self, tool_name: &str, input: Value) -> anyhow::Result<Value> {
        let skill = self
            .skills
            .iter()
            .find(|skill| {
                skill
                    .manifest
                    .tools
                    .iter()
                    .any(|tool| tool.name == tool_name)
            })
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?;

        let mut child = Command::new(&skill.manifest.entry.command)
            .args(&skill.manifest.entry.args)
            .current_dir(&skill.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("skill command stdin unavailable"))?;
        stdin
            .write_all(serde_json::to_vec(&input)?.as_slice())
            .await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            anyhow::bail!("skill command failed: {}", output.status);
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_and_executes_echo_skill() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let registry = SkillRegistry::load(skills_root).await.unwrap();
        let tools = registry.tools();
        assert!(tools.iter().any(|tool| tool.name == "echo"));

        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }))
            .await
            .unwrap();

        assert_eq!(result["text"], "hello");
    }
}
