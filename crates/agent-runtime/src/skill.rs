use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_availability::{
    SkillAvailability, SkillAvailabilityStatus, SkillCapabilityMetadata,
    evaluate_skill_availability,
};
use crate::skill_entry_resource::{ManifestEntryArgKind, classify_manifest_entry_arg};
use crate::skill_store_fs::PackageLimits;
use crate::tools::ToolPermission;
use crate::tools::process::read_limited_child_output;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: SkillCapabilityMetadata,
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
    #[serde(default = "default_tool_permission")]
    pub permission: ToolPermission,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct SkillExecutionContext {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub output_limit_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub(crate) root: PathBuf,
    pub(crate) manifest: SkillManifest,
    pub(crate) verification: Option<InstalledSkillVerification>,
    pub(crate) development_package_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct InstalledSkillVerification {
    pub(crate) expected_content_hash: String,
    pub(crate) limits: PackageLimits,
    pub(crate) execution_binding: Option<crate::skill_verified::VerifiedExecutionBinding>,
}

impl InstalledSkill {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledSkillStatus {
    pub id: String,
    pub description: String,
    pub availability: SkillAvailability,
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
    pub(crate) skills: Vec<InstalledSkill>,
    availability: Option<SkillAvailabilityContext>,
}

#[derive(Debug, Clone)]
struct SkillAvailabilityContext {
    platform: PlatformId,
    capabilities: CapabilitySet,
}

impl SkillRegistry {
    pub fn empty() -> Self {
        Self {
            skills: Vec::new(),
            availability: None,
        }
    }

    pub fn from_installed(skills: Vec<InstalledSkill>) -> anyhow::Result<Self> {
        crate::skill_runtime_source::validate_runtime_identities(&skills)?;
        Ok(Self {
            skills,
            availability: None,
        })
    }

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
        Self::from_installed(skills)
    }

    pub async fn load_development_skill(root: impl AsRef<Path>) -> anyhow::Result<InstalledSkill> {
        Self::load_skill(root.as_ref().to_path_buf()).await
    }

    pub(crate) async fn load_development_skill_for_package(
        root: impl AsRef<Path>,
        package_id: &crate::skill_package::SkillPackageId,
    ) -> anyhow::Result<InstalledSkill> {
        let mut skill = Self::load_skill(root.as_ref().to_path_buf()).await?;
        skill.development_package_id = Some(package_id.as_str().to_string());
        Ok(skill)
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
        Self::from_installed(skills)
    }

    pub fn tools(&self) -> Vec<SkillTool> {
        self.skills
            .iter()
            .filter(|skill| self.skill_is_available(skill))
            .flat_map(|skill| skill.manifest.tools.clone())
            .collect()
    }

    pub fn tools_with_skill_names(&self) -> Vec<(String, SkillTool)> {
        self.skills
            .iter()
            .filter(|skill| self.skill_is_available(skill))
            .flat_map(|skill| {
                skill
                    .manifest
                    .tools
                    .clone()
                    .into_iter()
                    .map(|tool| (skill.manifest.name.clone(), tool))
            })
            .collect()
    }

    pub fn installed_skill_statuses(&self) -> Vec<InstalledSkillStatus> {
        let mut statuses: Vec<_> = self
            .skills
            .iter()
            .map(|skill| InstalledSkillStatus {
                id: skill.manifest.name.clone(),
                description: skill.manifest.description.clone(),
                availability: self.skill_availability(skill),
            })
            .collect();
        statuses.sort_by(|left, right| left.id.cmp(&right.id));
        statuses
    }

    #[cfg(test)]
    pub fn empty_for_tests() -> Self {
        Self::empty()
    }

    pub fn with_platform_capabilities(
        mut self,
        platform: PlatformId,
        capabilities: CapabilitySet,
    ) -> Self {
        self.availability = Some(SkillAvailabilityContext {
            platform,
            capabilities,
        });
        self
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
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.execute_with_context(
            tool_name,
            input,
            SkillExecutionContext {
                workspace_root: cwd.clone(),
                cwd,
                output_limit_bytes,
            },
        )
        .await
    }

    pub async fn execute_with_context(
        &self,
        tool_name: &str,
        input: Value,
        context: SkillExecutionContext,
    ) -> anyhow::Result<Value> {
        let binding = self
            .resolve_runtime_tool_for_execution(tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?;
        self.execute_runtime_tool_with_context(&binding, input, context)
            .await
    }

    pub(crate) async fn execute_runtime_tool_with_context(
        &self,
        binding: &crate::skill_runtime_source::RuntimeToolBinding,
        input: Value,
        context: SkillExecutionContext,
    ) -> anyhow::Result<Value> {
        let skill = self
            .skills
            .get(binding.skill_index)
            .ok_or_else(|| anyhow::anyhow!("runtime tool owner is unavailable"))?;

        let availability = self.skill_availability(skill);
        if availability.status != SkillAvailabilityStatus::Available {
            anyhow::bail!("{}", availability.reason);
        }
        let prepared_execution = crate::skill_verified::prepare_before_execution(skill).await?;
        let (command, args, execution_root) = prepared_execution.as_ref().map_or_else(
            || {
                (
                    skill.manifest.entry.command.as_str(),
                    skill.manifest.entry.args.as_slice(),
                    skill.root.as_path(),
                )
            },
            |prepared| (prepared.command(), prepared.args(), prepared.current_dir()),
        );

        let mut child = Command::new(command)
            .args(args)
            .current_dir(execution_root)
            .env("GENERAL_AGENT_TOOL_NAME", &binding.local_name)
            .env("GENERAL_AGENT_WORKSPACE_ROOT", &context.workspace_root)
            .env("GENERAL_AGENT_CWD", &context.cwd)
            .env(
                "GENERAL_AGENT_OUTPUT_LIMIT_BYTES",
                context.output_limit_bytes.to_string(),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                return child_error(
                    &mut child,
                    anyhow::anyhow!("skill command stdout unavailable"),
                )
                .await;
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                return child_error(
                    &mut child,
                    anyhow::anyhow!("skill command stderr unavailable"),
                )
                .await;
            }
        };
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                return child_error(
                    &mut child,
                    anyhow::anyhow!("skill command stdin unavailable"),
                )
                .await;
            }
        };
        if let Err(error) = stdin
            .write_all(serde_json::to_vec(&input)?.as_slice())
            .await
        {
            return child_error(&mut child, error.into()).await;
        }
        drop(stdin);

        let output =
            match read_limited_child_output(stdout, stderr, context.output_limit_bytes).await {
                Ok(output) => output,
                Err(error) => return child_error(&mut child, error).await,
            };
        if output.stdout_truncated || output.stderr_truncated {
            return child_error(
                &mut child,
                anyhow::anyhow!("tool output exceeded runtime output limit"),
            )
            .await;
        }

        let status = match child.wait().await {
            Ok(status) => status,
            Err(error) => return child_error(&mut child, error.into()).await,
        };
        if !status.success() {
            anyhow::bail!("skill command failed: {}", status);
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }

    async fn load_skill(root: PathBuf) -> anyhow::Result<InstalledSkill> {
        let root = canonical_skill_root(&root).await?;
        let manifest_path =
            resolve_skill_package_file(&root, Path::new("skill.json"), "skill manifest").await?;
        let bytes = tokio::fs::read(&manifest_path).await.with_context(|| {
            format!("failed to read skill manifest {}", manifest_path.display())
        })?;
        let manifest: SkillManifest = serde_json::from_slice(&bytes).with_context(|| {
            format!("failed to parse skill manifest {}", manifest_path.display())
        })?;
        validate_manifest(&root, &manifest).await?;

        Ok(InstalledSkill {
            root,
            manifest,
            verification: None,
            development_package_id: None,
        })
    }

    pub(crate) fn skill_is_available(&self, skill: &InstalledSkill) -> bool {
        self.skill_availability(skill).status == SkillAvailabilityStatus::Available
    }

    fn skill_availability(&self, skill: &InstalledSkill) -> SkillAvailability {
        match &self.availability {
            Some(context) => evaluate_skill_availability(
                &skill.manifest.name,
                &skill.manifest.capabilities,
                context.platform,
                &context.capabilities,
                !skill.manifest.tools.is_empty(),
            ),
            None => SkillAvailability {
                skill_id: skill.manifest.name.clone(),
                status: SkillAvailabilityStatus::Available,
                missing_capabilities: Vec::new(),
                reason: "Available on this platform.".into(),
            },
        }
    }
}

async fn child_error<T>(
    child: &mut tokio::process::Child,
    primary: anyhow::Error,
) -> anyhow::Result<T> {
    match terminate_and_reap(child).await {
        Ok(()) => Err(primary),
        Err(reap) => {
            let primary_message = format!("{primary:#}");
            Err(primary.context(format!(
                "{primary_message}; process reap diagnostics: {reap:#}"
            )))
        }
    }
}

async fn terminate_and_reap(child: &mut tokio::process::Child) -> anyhow::Result<()> {
    let kill = child.start_kill();
    let wait = child.wait().await.map(|_| ());
    finish_reap(kill, wait)
}

pub(crate) fn finish_reap(
    kill: std::io::Result<()>,
    wait: std::io::Result<()>,
) -> anyhow::Result<()> {
    match (kill, wait) {
        (_, Ok(())) => Ok(()),
        (Ok(()), Err(wait)) => Err(wait).context("failed to wait for terminated skill process"),
        (Err(kill), Err(wait)) => anyhow::bail!(
            "failed to kill skill process: {kill}; failed to wait for skill process: {wait}"
        ),
    }
}

fn default_tool_permission() -> ToolPermission {
    ToolPermission::ReadWorkspace
}

pub(crate) async fn validate_manifest(root: &Path, manifest: &SkillManifest) -> anyhow::Result<()> {
    validate_manifest_semantics(manifest)?;
    validate_entry_resources(root, manifest).await
}

pub(crate) fn validate_manifest_semantics(manifest: &SkillManifest) -> anyhow::Result<()> {
    if manifest.name.trim().is_empty() {
        anyhow::bail!("skill manifest name must not be empty");
    }
    if manifest.description.trim().is_empty() {
        anyhow::bail!("skill manifest description must not be empty");
    }
    if manifest.version.trim().is_empty() {
        anyhow::bail!("skill manifest version must not be empty");
    }
    if manifest.entry.kind != "command" {
        anyhow::bail!("skill manifest entry type must be command");
    }
    if manifest.entry.command.trim().is_empty() {
        anyhow::bail!("skill manifest entry command must not be empty");
    }
    if manifest.tools.is_empty() {
        anyhow::bail!("skill manifest must define at least one runtime tool");
    }

    let mut tool_names = HashSet::new();
    for tool in &manifest.tools {
        validate_tool_name(&tool.name)?;
        if tool.description.trim().is_empty() {
            anyhow::bail!("skill manifest tool description must not be empty");
        }
        if tool.input_schema.get("type").and_then(Value::as_str) != Some("object") {
            anyhow::bail!("skill manifest tool input_schema type must be object");
        }
        if !tool_names.insert(tool.name.as_str()) {
            anyhow::bail!("skill manifest tool name must be unique: {}", tool.name);
        }
    }

    for arg in &manifest.entry.args {
        if classify_manifest_entry_arg(arg) == ManifestEntryArgKind::UnsafeRelative {
            anyhow::bail!("unsafe skill entry resource path: {arg}");
        }
    }

    Ok(())
}

fn validate_tool_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 || name.trim() != name || !is_tool_name(name) {
        anyhow::bail!("invalid runtime tool name: {name}");
    }

    Ok(())
}

fn is_tool_name(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

async fn validate_entry_resources(root: &Path, manifest: &SkillManifest) -> anyhow::Result<()> {
    for path in manifest_entry_resources(manifest) {
        resolve_skill_package_file(root, &path, "skill manifest entry resource").await?;
    }

    Ok(())
}

pub(crate) fn manifest_entry_resources(
    manifest: &SkillManifest,
) -> impl Iterator<Item = PathBuf> + '_ {
    manifest.entry.args.iter().filter_map(|arg| {
        let ManifestEntryArgKind::PackagedRelative(path) = classify_manifest_entry_arg(arg) else {
            return None;
        };
        Some(path)
    })
}

pub(crate) async fn canonical_skill_root(root: &Path) -> anyhow::Result<PathBuf> {
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to inspect skill package root {}", root.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "skill package root must not be a symlink: {}",
            root.display()
        );
    }
    if !metadata.is_dir() {
        anyhow::bail!("skill package root must be a directory: {}", root.display());
    }
    tokio::fs::canonicalize(root)
        .await
        .with_context(|| format!("failed to resolve skill package root {}", root.display()))
}

async fn resolve_skill_package_file(
    canonical_root: &Path,
    relative: &Path,
    label: &str,
) -> anyhow::Result<PathBuf> {
    let candidate = canonical_root.join(relative);
    let metadata = match tokio::fs::symlink_metadata(&candidate).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            anyhow::bail!("{label} does not exist: {}", relative.display());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {label} {}", candidate.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        anyhow::bail!("{label} must not be a symlink: {}", candidate.display());
    }
    if !metadata.is_file() {
        anyhow::bail!("{label} must be a regular file: {}", candidate.display());
    }
    let canonical = tokio::fs::canonicalize(&candidate)
        .await
        .with_context(|| format!("failed to resolve {label} {}", candidate.display()))?;
    if !canonical.starts_with(canonical_root) {
        anyhow::bail!(
            "{label} escapes skill package root: {}",
            candidate.display()
        );
    }
    Ok(canonical)
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
    async fn load_development_skill_validates_one_runtime_package() {
        let root = unique_test_dir("single-runtime-package");
        write_echo_skill(&root, "echo", "echo").await;

        let skill = SkillRegistry::load_development_skill(root.join("echo"))
            .await
            .unwrap();

        assert_eq!(skill.manifest.name, "echo");
        assert_eq!(skill.manifest.tools[0].name, "echo");
        remove_test_dir(root).await;
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

    #[tokio::test]
    async fn rejects_manifest_with_unsupported_entry_type() {
        let root = unique_test_dir("invalid-entry-type");
        write_skill_manifest(
            &root,
            "bad-entry",
            serde_json::json!({
                "name": "bad-entry",
                "description": "Invalid entry type.",
                "version": "0.1.0",
                "entry": {
                    "type": "http",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": "bad_entry",
                        "description": "Invalid entry type.",
                        "input_schema": { "type": "object" }
                    }
                ]
            }),
        )
        .await;

        let error = SkillRegistry::load_development(&root).await.unwrap_err();

        assert!(error.to_string().contains("entry type must be command"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn rejects_manifest_with_missing_entry_resource() {
        let root = unique_test_dir("missing-entry-resource");
        write_skill_manifest(
            &root,
            "missing-entry",
            serde_json::json!({
                "name": "missing-entry",
                "description": "Missing entry file.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["missing.js"]
                },
                "tools": [
                    {
                        "name": "missing_entry",
                        "description": "Missing entry file.",
                        "input_schema": { "type": "object" }
                    }
                ]
            }),
        )
        .await;

        let error = SkillRegistry::load_development(&root).await.unwrap_err();

        assert!(error.to_string().contains("entry resource does not exist"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn rejects_manifest_with_invalid_tool_name() {
        let root = unique_test_dir("invalid-tool-name");
        write_skill_manifest(
            &root,
            "invalid-tool-name",
            serde_json::json!({
                "name": "invalid-tool-name",
                "description": "Invalid tool name.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": "bad tool",
                        "description": "Invalid tool name.",
                        "input_schema": { "type": "object" }
                    }
                ]
            }),
        )
        .await;
        tokio::fs::write(
            root.join("invalid-tool-name").join("index.js"),
            "process.stdin.resume();\n",
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_development(&root).await.unwrap_err();

        assert!(error.to_string().contains("invalid runtime tool name"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn accepts_duplicate_local_names_across_distinct_packaged_skills() {
        let root = unique_test_dir("duplicate-tools");
        write_echo_skill(&root, "first", "echo").await;
        write_echo_skill(&root, "second", "echo").await;
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "first" },
                    { "path": "second" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let registry = SkillRegistry::load_packaged(&root).await.unwrap();
        let tools = registry.tools_with_runtime_sources();

        assert!(tools.iter().any(|tool| tool.canonical_id == "first/echo"));
        assert!(tools.iter().any(|tool| tool.canonical_id == "second/echo"));
        assert!(registry.resolve_runtime_tool("echo").is_none());
        remove_test_dir(root).await;
    }

    async fn write_echo_skill(root: &Path, folder: &str, tool_name: &str) {
        write_skill_manifest(
            root,
            folder,
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
            }),
        )
        .await;
        tokio::fs::write(
            root.join(folder).join("index.js"),
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await
        .unwrap();
    }

    async fn write_skill_manifest(root: &Path, folder: &str, manifest: Value) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("skill.json"), manifest.to_string())
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
