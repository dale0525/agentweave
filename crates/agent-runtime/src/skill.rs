use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
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

#[derive(Debug, Clone, Deserialize)]
struct SkillBundleIndex {
    skills: Vec<SkillBundleEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct SkillBundleEntry {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    skills: Vec<InstalledSkill>,
}

impl SkillRegistry {
    pub async fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load_development(root).await
    }

    pub async fn load_development(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut skills = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let manifest_path = path.join("skill.json");
            if !manifest_path.is_file() {
                continue;
            }

            skills.push(Self::load_skill(path).await?);
        }

        Ok(Self { skills })
    }

    pub async fn load_packaged(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let bytes = tokio::fs::read(root.join("skill-bundle.json"))
            .await
            .with_context(|| {
                format!("failed to read packaged skill index in {}", root.display())
            })?;
        let index: SkillBundleIndex = serde_json::from_slice(&bytes)?;
        let mut skills = Vec::new();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve packaged skill root {}", root.display()))?;

        for entry in index.skills {
            let skill_root =
                resolve_packaged_skill_path(root, &canonical_root, &entry.path).await?;
            skills.push(Self::load_skill(skill_root).await?);
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
        self.execute_with_output_limit(tool_name, input, usize::MAX)
            .await
    }

    pub async fn execute_with_output_limit(
        &self,
        tool_name: &str,
        input: Value,
        output_limit_bytes: usize,
    ) -> anyhow::Result<Value> {
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
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("skill command stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("skill command stderr unavailable"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("skill command stdin unavailable"))?;
        stdin
            .write_all(serde_json::to_vec(&input)?.as_slice())
            .await?;
        drop(stdin);

        let output = read_limited_child_output(stdout, stderr, output_limit_bytes).await?;
        if output.stdout_truncated || output.stderr_truncated {
            child.kill().await?;
            let _ = child.wait().await;
            anyhow::bail!("tool output exceeded runtime output limit");
        }

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("skill command failed: {}", status);
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }

    async fn load_skill(root: PathBuf) -> anyhow::Result<InstalledSkill> {
        let manifest_path = root.join("skill.json");
        let bytes = tokio::fs::read(&manifest_path).await.with_context(|| {
            format!("failed to read skill manifest {}", manifest_path.display())
        })?;
        let manifest: SkillManifest = serde_json::from_slice(&bytes).with_context(|| {
            format!("failed to parse skill manifest {}", manifest_path.display())
        })?;
        validate_manifest(&manifest)?;

        Ok(InstalledSkill { root, manifest })
    }
}

struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

struct LimitedChildOutput {
    stdout: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

async fn read_limited_child_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedChildOutput> {
    let stdout_future = read_limited_stream(stdout, output_limit_bytes);
    let stderr_future = read_limited_stream(stderr, output_limit_bytes);
    tokio::pin!(stdout_future);
    tokio::pin!(stderr_future);

    let mut stdout_output: Option<LimitedOutput> = None;
    let mut stderr_output: Option<LimitedOutput> = None;

    while stdout_output.is_none() || stderr_output.is_none() {
        tokio::select! {
            output = &mut stdout_future, if stdout_output.is_none() => {
                stdout_output = Some(output?);
            }
            output = &mut stderr_future, if stderr_output.is_none() => {
                stderr_output = Some(output?);
            }
        }

        let stdout_truncated = stdout_output
            .as_ref()
            .map(|output| output.truncated)
            .unwrap_or(false);
        let stderr_truncated = stderr_output
            .as_ref()
            .map(|output| output.truncated)
            .unwrap_or(false);
        if stdout_truncated || stderr_truncated {
            return Ok(LimitedChildOutput {
                stdout: stdout_output.map(|output| output.bytes).unwrap_or_default(),
                stdout_truncated,
                stderr_truncated,
            });
        }
    }

    let stdout = stdout_output.expect("stdout output should be captured");
    let stderr = stderr_output.expect("stderr output should be captured");
    Ok(LimitedChildOutput {
        stdout: stdout.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

async fn read_limited_stream(
    mut stream: impl AsyncRead + Unpin,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedOutput> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let hard_limit = output_limit_bytes.saturating_add(1);

    loop {
        let remaining = hard_limit.saturating_sub(bytes.len());
        if remaining == 0 {
            return Ok(LimitedOutput {
                bytes,
                truncated: true,
            });
        }

        let read_len = remaining.min(buffer.len());
        let read = stream.read(&mut buffer[..read_len]).await?;
        if read == 0 {
            return Ok(LimitedOutput {
                bytes,
                truncated: false,
            });
        }

        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > output_limit_bytes {
            bytes.truncate(output_limit_bytes);
            return Ok(LimitedOutput {
                bytes,
                truncated: true,
            });
        }
    }
}

fn validate_manifest(manifest: &SkillManifest) -> anyhow::Result<()> {
    if manifest.name.trim().is_empty() {
        anyhow::bail!("skill manifest name must not be empty");
    }
    if manifest.version.trim().is_empty() {
        anyhow::bail!("skill manifest version must not be empty");
    }
    if manifest.entry.command.trim().is_empty() {
        anyhow::bail!("skill manifest entry command must not be empty");
    }
    if manifest.tools.is_empty() {
        anyhow::bail!("skill manifest must define at least one runtime tool");
    }

    let mut tool_names = HashSet::new();
    for tool in &manifest.tools {
        if tool.name.trim().is_empty() {
            anyhow::bail!("skill manifest tool name must not be empty");
        }
        if !tool_names.insert(tool.name.as_str()) {
            anyhow::bail!("skill manifest tool name must be unique: {}", tool.name);
        }
    }

    Ok(())
}

fn is_safe_packaged_skill_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

async fn resolve_packaged_skill_path(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> anyhow::Result<PathBuf> {
    if !is_safe_packaged_skill_path(path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn development_load_scans_skill_directories() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let registry = SkillRegistry::load_development(skills_root).await.unwrap();
        let tools = registry.tools();
        assert!(tools.iter().any(|tool| tool.name == "echo"));

        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }))
            .await
            .unwrap();

        assert_eq!(result["text"], "hello");
    }

    #[tokio::test]
    async fn default_load_keeps_development_discovery_for_existing_callers() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let registry = SkillRegistry::load(skills_root).await.unwrap();

        assert!(registry.tools().iter().any(|tool| tool.name == "echo"));
    }

    #[tokio::test]
    async fn packaged_load_uses_only_the_frozen_skill_index() {
        let root = unique_test_dir("packaged-load");
        write_echo_skill(&root, "included", "included_echo").await;
        write_echo_skill(&root, "unlisted", "unlisted_echo").await;
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "included" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let registry = SkillRegistry::load_packaged(&root).await.unwrap();
        let tool_names: Vec<_> = registry.tools().into_iter().map(|tool| tool.name).collect();

        assert_eq!(tool_names, vec!["included_echo"]);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn packaged_load_rejects_unsafe_index_paths() {
        let root = unique_test_dir("packaged-unsafe");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "../echo" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_packaged(&root).await.unwrap_err();

        assert!(error.to_string().contains("unsafe packaged skill path"));
        remove_test_dir(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn packaged_load_rejects_symlink_escape_paths() {
        let root = unique_test_dir("packaged-symlink");
        let outside_root = unique_test_dir("packaged-outside");
        let outside_skill_dir = outside_root.join("outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        write_echo_skill(&outside_root, "outside", "outside_echo").await;

        if let Err(error) = create_dir_symlink(&outside_skill_dir, &root.join("included")) {
            remove_test_dir(root).await;
            remove_test_dir(outside_root).await;
            panic!("symlink creation failed on unix: {error}");
        }

        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "included" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_packaged(&root).await.unwrap_err();

        assert!(error.to_string().contains("unsafe packaged skill path"));
        remove_test_dir(root).await;
        remove_test_dir(outside_root).await;
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn packaged_load_rejects_symlink_escape_paths() {
        eprintln!("skipping symlink escape test on non-unix platform");
    }

    #[tokio::test]
    async fn rejects_manifest_without_runtime_tools() {
        let root = unique_test_dir("invalid-manifest");
        let skill_dir = root.join("empty");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": "empty",
                "description": "Invalid empty runtime skill.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": []
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_development(&root).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("must define at least one runtime tool")
        );
        remove_test_dir(root).await;
    }

    async fn write_echo_skill(root: &Path, folder: &str, tool_name: &str) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
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
        tokio::fs::write(
            skill_dir.join("index.js"),
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await
        .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
