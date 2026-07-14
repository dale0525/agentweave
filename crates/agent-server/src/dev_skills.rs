use agent_runtime::{
    skill::{SkillManifest, SkillRegistry},
    skill_catalog::SkillCatalog,
    skill_package::SkillPackageKind,
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    path::{Component, Path, PathBuf},
};

const PACKAGE_METADATA_FILE: &str = "agentweave.json";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillInventory {
    pub root: String,
    pub packages: Vec<DevSkillPackage>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillPackage {
    pub id: String,
    pub path: String,
    pub name: String,
    pub description: String,
    pub has_skill_md: bool,
    pub has_runtime_manifest: bool,
    pub runtime_tools: Vec<String>,
    pub package_kind: DevSkillPackageKind,
    pub bundle_ready: bool,
    pub runtime_ready: bool,
    pub instruction_ready: bool,
    pub release_ready: bool,
    pub readiness_issues: Vec<String>,
    pub required_runtime_tools: Vec<String>,
    pub required_connectors: Vec<String>,
    pub has_package_metadata: bool,
    pub validation: DevSkillValidation,
    #[serde(skip)]
    instruction_skill_name: Option<String>,
    #[serde(skip)]
    declared_kind: Option<SkillPackageKind>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DevSkillPackageKind {
    Runtime,
    Instruction,
    Combined,
    Empty,
    Invalid,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillValidation {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillPackageMetadata {
    #[serde(default)]
    pub kind: Option<SkillPackageKind>,
    #[serde(default)]
    pub package: DevSkillPackageTargets,
    #[serde(default)]
    pub requires: DevSkillPackageRequirements,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillPackageTargets {
    pub include_runtime: Option<bool>,
    pub include_instructions: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillPackageRequirements {
    #[serde(default)]
    pub runtime_tools: Vec<String>,
    #[serde(default)]
    pub connectors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SkillPackageMetadata {
    name: Option<String>,
    description: Option<String>,
    runtime_tools: Vec<String>,
    instruction_skill_name: Option<String>,
    package_metadata: DevSkillPackageMetadata,
    has_package_metadata: bool,
    validation: DevSkillValidation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillPackageReleaseError {
    pub inventory: DevSkillInventory,
    message: String,
}

impl SkillPackageReleaseError {
    fn not_ready(inventory: DevSkillInventory) -> Self {
        Self {
            inventory,
            message: "skill packages are not release ready".to_string(),
        }
    }

    fn scan_failed(error: anyhow::Error) -> Self {
        Self {
            inventory: DevSkillInventory {
                root: String::new(),
                packages: Vec::new(),
            },
            message: format!("failed to scan skill packages: {error}"),
        }
    }
}

impl fmt::Display for SkillPackageReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SkillPackageReleaseError {}

pub async fn scan_skill_packages(root: impl AsRef<Path>) -> anyhow::Result<DevSkillInventory> {
    let root = root.as_ref();
    let canonical_root = ensure_skills_root(root).await?;
    let mut packages = Vec::new();
    let mut entries = tokio::fs::read_dir(&canonical_root).await?;

    while let Some(entry) = entries.next_entry().await? {
        let package_path = entry.path();
        if !is_safe_package_entry_for_scan(&canonical_root, &package_path).await? {
            continue;
        }
        packages.push(scan_one_package(&canonical_root, package_path).await);
    }

    packages.sort_by(|left, right| left.id.cmp(&right.id));
    apply_duplicate_diagnostics(&mut packages);

    Ok(DevSkillInventory {
        root: canonical_root.display().to_string(),
        packages,
    })
}

pub async fn check_skill_packages(
    root: impl AsRef<Path>,
) -> Result<DevSkillInventory, SkillPackageReleaseError> {
    let inventory = scan_skill_packages(root)
        .await
        .map_err(SkillPackageReleaseError::scan_failed)?;
    if inventory
        .packages
        .iter()
        .all(|package| package.release_ready)
    {
        Ok(inventory)
    } else {
        Err(SkillPackageReleaseError::not_ready(inventory))
    }
}

#[allow(dead_code)]
pub async fn delete_skill_package(
    root: impl AsRef<Path>,
    id: &str,
) -> anyhow::Result<DevSkillInventory> {
    let root = root.as_ref();
    let canonical_root = ensure_skills_root(root).await?;
    validate_package_id(id)?;
    let target = canonical_root.join(id);
    match classify_package_entry_for_delete(&canonical_root, &target, id).await? {
        PackageEntryKind::Directory => tokio::fs::remove_dir_all(&target).await?,
        PackageEntryKind::Symlink => tokio::fs::remove_file(&target).await?,
    }
    scan_skill_packages(&canonical_root).await
}

async fn ensure_skills_root(root: &Path) -> anyhow::Result<PathBuf> {
    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
    let metadata = tokio::fs::metadata(&canonical_root)
        .await
        .with_context(|| format!("failed to read skill root metadata {}", root.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!(
            "skill root is not a directory: {}",
            canonical_root.display()
        );
    }
    Ok(canonical_root)
}

async fn is_safe_package_entry_for_scan(root: &Path, package_path: &Path) -> anyhow::Result<bool> {
    let file_type = tokio::fs::symlink_metadata(package_path)
        .await
        .with_context(|| format!("failed to read package entry {}", package_path.display()))?
        .file_type();
    if file_type.is_dir() {
        return Ok(true);
    }
    if !file_type.is_symlink() {
        return Ok(false);
    }

    let canonical_path = match tokio::fs::canonicalize(package_path).await {
        Ok(path) => path,
        Err(_) => return Ok(false),
    };
    if !canonical_path.starts_with(root) {
        return Ok(false);
    }

    Ok(tokio::fs::metadata(&canonical_path)
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageEntryKind {
    Directory,
    Symlink,
}

async fn classify_package_entry_for_delete(
    root: &Path,
    target: &Path,
    id: &str,
) -> anyhow::Result<PackageEntryKind> {
    let metadata = tokio::fs::symlink_metadata(target)
        .await
        .with_context(|| format!("failed to read skill package entry {}", target.display()))?;
    let file_type = metadata.file_type();
    if !file_type.is_dir() && !file_type.is_symlink() {
        anyhow::bail!("skill package is not a directory: {id}");
    }

    let canonical_target = tokio::fs::canonicalize(target)
        .await
        .with_context(|| format!("failed to resolve skill package {}", target.display()))?;
    if !canonical_target.starts_with(root) {
        anyhow::bail!("unsafe skill package path: {id}");
    }
    if !tokio::fs::metadata(&canonical_target).await?.is_dir() {
        anyhow::bail!("skill package is not a directory: {id}");
    }

    if file_type.is_symlink() {
        Ok(PackageEntryKind::Symlink)
    } else {
        Ok(PackageEntryKind::Directory)
    }
}

async fn scan_one_package(root: &Path, package_path: PathBuf) -> DevSkillPackage {
    let relative_path = package_path
        .strip_prefix(root)
        .unwrap_or(package_path.as_path())
        .to_path_buf();
    let id = relative_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative_path.display().to_string());
    let skill_md_path = package_path.join("SKILL.md");
    let runtime_manifest_path = package_path.join("skill.json");
    let has_skill_md = path_is_file(&skill_md_path).await;
    let has_runtime_manifest = path_is_file(&runtime_manifest_path).await;
    let package_metadata_path = package_path.join(PACKAGE_METADATA_FILE);
    let metadata = collect_package_metadata(
        root,
        &package_path,
        &skill_md_path,
        &package_metadata_path,
        has_skill_md,
        has_runtime_manifest,
    )
    .await;
    let ok = metadata.validation.ok;
    let package_kind = compute_package_kind(has_skill_md, has_runtime_manifest, ok);
    let required_runtime_tools = metadata.package_metadata.requires.runtime_tools.clone();
    let required_connectors = metadata.package_metadata.requires.connectors.clone();

    DevSkillPackage {
        id: id.clone(),
        path: relative_path.display().to_string(),
        name: metadata.name.unwrap_or_else(|| id.clone()),
        description: metadata
            .description
            .unwrap_or_else(|| "No skill metadata found.".to_string()),
        has_skill_md,
        has_runtime_manifest,
        runtime_tools: metadata.runtime_tools,
        package_kind,
        bundle_ready: false,
        runtime_ready: false,
        instruction_ready: false,
        release_ready: false,
        readiness_issues: Vec::new(),
        required_runtime_tools,
        required_connectors,
        has_package_metadata: metadata.has_package_metadata,
        validation: metadata.validation,
        instruction_skill_name: metadata.instruction_skill_name,
        declared_kind: metadata.package_metadata.kind,
    }
}

fn apply_duplicate_diagnostics(packages: &mut [DevSkillPackage]) {
    let mut runtime_tools = HashMap::<String, Vec<usize>>::new();
    let mut instruction_names = HashMap::<String, Vec<usize>>::new();

    for (index, package) in packages.iter().enumerate() {
        for tool_name in &package.runtime_tools {
            runtime_tools
                .entry(tool_name.clone())
                .or_default()
                .push(index);
        }
        if let Some(skill_name) = package.instruction_skill_name.as_ref() {
            instruction_names
                .entry(skill_name.clone())
                .or_default()
                .push(index);
        }
    }

    for (tool_name, owners) in runtime_tools {
        if owners.len() < 2 {
            continue;
        }
        for index in owners {
            packages[index]
                .validation
                .errors
                .push(format!("duplicate runtime tool name: {tool_name}"));
        }
    }

    for (skill_name, owners) in instruction_names {
        if owners.len() < 2 {
            continue;
        }
        for index in owners {
            packages[index]
                .validation
                .errors
                .push(format!("duplicate instruction skill name: {skill_name}"));
        }
    }

    apply_readiness(packages);
}

fn apply_readiness(packages: &mut [DevSkillPackage]) {
    for package in packages.iter_mut() {
        package.validation.ok = package.validation.errors.is_empty();
        package.package_kind = compute_package_kind(
            package.has_skill_md,
            package.has_runtime_manifest,
            package.validation.ok,
        );
    }

    let available_runtime_tools = packages
        .iter()
        .filter(|package| {
            package.validation.ok
                && package.has_runtime_manifest
                && !package.runtime_tools.is_empty()
        })
        .flat_map(|package| package.runtime_tools.iter().cloned())
        .collect::<BTreeSet<_>>();

    for package in packages {
        let mut readiness_issues = Vec::new();
        let runtime_ready = package.validation.ok
            && package.has_runtime_manifest
            && !package.runtime_tools.is_empty();

        if package.has_runtime_manifest && package.runtime_tools.is_empty() {
            readiness_issues.push("runtime manifest does not define any tools".to_string());
        }

        let mut instruction_ready =
            package.validation.ok && package.has_skill_md && package.has_package_metadata;
        let host_tools_only = package.declared_kind == Some(SkillPackageKind::HostToolsOnly);
        if package.has_skill_md {
            if !package.has_package_metadata {
                readiness_issues.push(format!(
                    "missing {PACKAGE_METADATA_FILE} metadata for instruction skill"
                ));
            }
            if !host_tools_only {
                for required_tool in &package.required_runtime_tools {
                    if !available_runtime_tools.contains(required_tool) {
                        instruction_ready = false;
                        readiness_issues
                            .push(format!("missing required runtime tool: {required_tool}"));
                    }
                }
                for required_connector in &package.required_connectors {
                    instruction_ready = false;
                    readiness_issues
                        .push(format!("missing required connector: {required_connector}"));
                }
            }
        }

        let has_package_assets = package.has_runtime_manifest || package.has_skill_md;
        if !has_package_assets {
            readiness_issues.push("package does not contain SKILL.md or skill.json".to_string());
        }

        let release_ready = package.validation.ok
            && has_package_assets
            && (!package.has_runtime_manifest || runtime_ready)
            && (!package.has_skill_md || instruction_ready);

        package.runtime_ready = runtime_ready;
        package.instruction_ready = instruction_ready;
        package.release_ready = release_ready;
        package.bundle_ready = release_ready;
        package.readiness_issues = readiness_issues;
    }
}

#[allow(dead_code)]
fn validate_package_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() {
        anyhow::bail!("skill package id must not be empty");
    }
    if id.contains('/') || id.contains('\\') {
        anyhow::bail!("skill package id must be a single path segment: {id}");
    }

    let mut components = Path::new(id).components();
    let Some(Component::Normal(component)) = components.next() else {
        anyhow::bail!("skill package id must be a single path segment: {id}");
    };
    if component != std::ffi::OsStr::new(id) || components.next().is_some() {
        anyhow::bail!("skill package id must be a single path segment: {id}");
    }
    Ok(())
}

fn compute_package_kind(
    has_skill_md: bool,
    has_runtime_manifest: bool,
    ok: bool,
) -> DevSkillPackageKind {
    if !ok {
        return DevSkillPackageKind::Invalid;
    }
    match (has_skill_md, has_runtime_manifest) {
        (true, true) => DevSkillPackageKind::Combined,
        (true, false) => DevSkillPackageKind::Instruction,
        (false, true) => DevSkillPackageKind::Runtime,
        (false, false) => DevSkillPackageKind::Empty,
    }
}

async fn collect_package_metadata(
    root: &Path,
    package_path: &Path,
    skill_md_path: &Path,
    package_metadata_path: &Path,
    has_skill_md: bool,
    has_runtime_manifest: bool,
) -> SkillPackageMetadata {
    let mut name = None;
    let mut description = None;
    let mut runtime_tools = Vec::new();
    let mut instruction_skill_name = None;
    let mut errors = Vec::new();
    let has_package_metadata = path_is_file(package_metadata_path).await;
    let package_metadata = read_package_metadata(package_metadata_path)
        .await
        .unwrap_or_else(|error| {
            errors.push(error);
            DevSkillPackageMetadata::default()
        });

    if has_runtime_manifest {
        let runtime_package = tokio::fs::canonicalize(package_path)
            .await
            .with_context(|| format!("failed to resolve skill package {}", package_path.display()))
            .and_then(|path| {
                if path.starts_with(root) {
                    Ok(path)
                } else {
                    anyhow::bail!("unsafe skill package path: {}", package_path.display())
                }
            });
        match runtime_package {
            Ok(path) => match SkillRegistry::load_development_skill(path).await {
                Ok(skill) => {
                    let manifest = skill.manifest();
                    name = Some(manifest.name.clone());
                    description = Some(manifest.description.clone());
                    runtime_tools = runtime_tool_names(manifest);
                }
                Err(error) => errors.push(error.to_string()),
            },
            Err(error) => errors.push(error.to_string()),
        }
    }

    if has_skill_md {
        match SkillCatalog::read_development_skill_summary(root, skill_md_path).await {
            Ok(summary) => {
                instruction_skill_name = Some(summary.name.clone());
                if name.is_none() {
                    name = Some(summary.name.clone());
                }
                if description.is_none() {
                    description = Some(summary.description.clone());
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
    }

    SkillPackageMetadata {
        name,
        description,
        runtime_tools,
        instruction_skill_name,
        package_metadata,
        has_package_metadata,
        validation: DevSkillValidation {
            ok: errors.is_empty(),
            errors,
            warnings: Vec::new(),
        },
    }
}

async fn read_package_metadata(path: &Path) -> Result<DevSkillPackageMetadata, String> {
    if !path_is_file(path).await {
        return Ok(DevSkillPackageMetadata::default());
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("failed to read {PACKAGE_METADATA_FILE}: {error}"))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse {PACKAGE_METADATA_FILE}: {error}"))
}

fn runtime_tool_names(manifest: &SkillManifest) -> Vec<String> {
    manifest
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

async fn path_is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tokio::fs;

    #[tokio::test]
    async fn scan_reports_package_kinds_and_partial_errors() {
        let root = unique_test_dir("scan-kinds");
        write_runtime_skill(&root, "runtime-only", "runtime-only", "runtime_echo").await;
        write_instruction_skill(&root, "instruction-only", "planning", "Plan work.").await;
        write_runtime_skill(&root, "combined", "combined", "combined_echo").await;
        write_instruction_skill(&root, "combined", "combined", "Combined instructions.").await;
        fs::create_dir_all(root.join("empty")).await.unwrap();
        fs::create_dir_all(root.join("invalid")).await.unwrap();
        fs::write(root.join("invalid").join("skill.json"), "{not json")
            .await
            .unwrap();

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert_eq!(
            packages["runtime-only"].package_kind,
            DevSkillPackageKind::Runtime
        );
        assert_eq!(
            packages["instruction-only"].package_kind,
            DevSkillPackageKind::Instruction
        );
        assert_eq!(
            packages["combined"].package_kind,
            DevSkillPackageKind::Combined
        );
        assert_eq!(packages["empty"].package_kind, DevSkillPackageKind::Empty);
        assert_eq!(
            packages["invalid"].package_kind,
            DevSkillPackageKind::Invalid
        );
        assert!(packages["runtime-only"].bundle_ready);
        assert!(packages["runtime-only"].runtime_ready);
        assert!(!packages["runtime-only"].instruction_ready);
        assert!(packages["runtime-only"].release_ready);
        assert!(packages["instruction-only"].bundle_ready);
        assert!(!packages["instruction-only"].runtime_ready);
        assert!(packages["instruction-only"].instruction_ready);
        assert!(packages["instruction-only"].release_ready);
        assert!(!packages["invalid"].validation.ok);
        assert!(!packages["invalid"].release_ready);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn instruction_dependency_on_project_runtime_tool_controls_readiness() {
        let root = unique_test_dir("instruction-runtime-dependencies");
        write_runtime_skill(&root, "filesystem", "filesystem", "read_text_file").await;
        write_instruction_skill(&root, "reader", "reader", "Read project files.").await;
        write_package_metadata(
            &root,
            "reader",
            json!({
                "requires": {
                    "runtimeTools": ["read_text_file"]
                }
            }),
        )
        .await;
        write_instruction_skill(&root, "browser", "browser", "Use a browser.").await;
        write_package_metadata(
            &root,
            "browser",
            json!({
                "requires": {
                    "runtimeTools": ["open_browser"]
                }
            }),
        )
        .await;

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(packages["reader"].validation.ok);
        assert!(packages["reader"].instruction_ready);
        assert!(packages["reader"].release_ready);
        assert!(packages["reader"].readiness_issues.is_empty());
        assert!(packages["browser"].validation.ok);
        assert!(!packages["browser"].instruction_ready);
        assert!(!packages["browser"].release_ready);
        assert!(
            packages["browser"]
                .readiness_issues
                .iter()
                .any(|issue| issue.contains("missing required runtime tool: open_browser"))
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn instruction_skill_without_project_metadata_is_not_ready() {
        let root = unique_test_dir("instruction-metadata-required");
        write_instruction_skill_without_metadata(&root, "planning", "planning", "Plan work.").await;

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(packages["planning"].validation.ok);
        assert!(!packages["planning"].instruction_ready);
        assert!(!packages["planning"].release_ready);
        assert!(
            packages["planning"]
                .readiness_issues
                .iter()
                .any(|issue| issue.contains("missing agentweave.json metadata"))
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn instruction_dependency_on_connector_is_not_ready_until_supported() {
        let root = unique_test_dir("instruction-connector-dependencies");
        write_instruction_skill(&root, "linear", "linear", "Manage Linear issues.").await;
        write_package_metadata(
            &root,
            "linear",
            json!({
                "requires": {
                    "connectors": ["linear"]
                }
            }),
        )
        .await;

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(packages["linear"].validation.ok);
        assert!(!packages["linear"].instruction_ready);
        assert!(!packages["linear"].release_ready);
        assert!(
            packages["linear"]
                .readiness_issues
                .iter()
                .any(|issue| issue.contains("missing required connector: linear"))
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn release_check_fails_when_any_package_is_not_ready() {
        let root = unique_test_dir("release-check");
        write_instruction_skill(&root, "ready", "ready", "Ready instructions.").await;
        write_instruction_skill(&root, "not-ready", "not-ready", "Needs a connector.").await;
        write_package_metadata(
            &root,
            "not-ready",
            json!({
                "requires": {
                    "connectors": ["browser"]
                }
            }),
        )
        .await;

        let error = check_skill_packages(&root).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("skill packages are not release ready")
        );
        assert!(
            error
                .inventory
                .packages
                .iter()
                .any(|package| package.id == "not-ready" && !package.release_ready)
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn scan_reports_duplicate_runtime_tools_and_instruction_names() {
        let root = unique_test_dir("scan-duplicates");
        write_runtime_skill(&root, "runtime-a", "runtime-a", "shared_tool").await;
        write_runtime_skill(&root, "runtime-b", "runtime-b", "shared_tool").await;
        write_instruction_skill(&root, "instruction-a", "shared", "First.").await;
        write_instruction_skill(&root, "instruction-b", "shared", "Second.").await;

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(
            packages["runtime-a"]
                .validation
                .errors
                .iter()
                .any(|error| error.contains("duplicate runtime tool name: shared_tool"))
        );
        assert!(
            packages["instruction-b"]
                .validation
                .errors
                .iter()
                .any(|error| error.contains("duplicate instruction skill name: shared"))
        );
        remove_test_dir(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scan_includes_symlinked_package_inside_root() {
        let root = unique_test_dir("scan-safe-symlink");
        let storage_root = root.join("storage");
        write_runtime_skill(&storage_root, "target-package", "target-package", "echo").await;
        create_dir_symlink(
            storage_root.join("target-package"),
            root.join("linked-package"),
        );

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert_eq!(
            packages["linked-package"].package_kind,
            DevSkillPackageKind::Runtime
        );
        assert!(packages["linked-package"].has_runtime_manifest);
        assert_eq!(packages["linked-package"].runtime_tools, vec!["echo"]);
        remove_test_dir(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_symlinked_package_removes_link_only() {
        let root = unique_test_dir("delete-safe-symlink");
        let storage_root = root.join("storage");
        write_runtime_skill(&storage_root, "target-package", "target-package", "echo").await;
        create_dir_symlink(
            storage_root.join("target-package"),
            root.join("linked-package"),
        );

        let inventory = delete_skill_package(&root, "linked-package").await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(!packages.contains_key("linked-package"));
        assert!(!root.join("linked-package").exists());
        assert!(
            root.join("storage")
                .join("target-package")
                .join("skill.json")
                .exists()
        );
        remove_test_dir(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_is_not_scanned_or_deleted() {
        let root = unique_test_dir("scan-escape-symlink");
        let outside_root = unique_test_dir("scan-escape-target");
        write_runtime_skill(&outside_root, "outside-package", "outside-package", "echo").await;
        fs::create_dir_all(&root).await.unwrap();
        create_dir_symlink(
            outside_root.join("outside-package"),
            root.join("escape-package"),
        );

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);
        assert!(!packages.contains_key("escape-package"));

        let error = delete_skill_package(&root, "escape-package")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unsafe skill package path"));
        assert!(root.join("escape-package").exists());
        assert!(
            outside_root
                .join("outside-package")
                .join("skill.json")
                .exists()
        );

        remove_test_dir(root).await;
        remove_test_dir(outside_root).await;
    }

    fn packages_by_id(inventory: &DevSkillInventory) -> BTreeMap<String, DevSkillPackage> {
        inventory
            .packages
            .iter()
            .cloned()
            .map(|package| (package.id.clone(), package))
            .collect()
    }

    async fn write_runtime_skill(root: &Path, folder: &str, name: &str, tool_name: &str) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(
            skill_dir.join("skill.json"),
            json!({
                "name": name,
                "description": format!("{name} runtime skill."),
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": tool_name,
                        "description": format!("{tool_name} tool."),
                        "input_schema": { "type": "object" }
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

    async fn write_instruction_skill(root: &Path, folder: &str, name: &str, description: &str) {
        write_instruction_skill_without_metadata(root, folder, name, description).await;
        write_package_metadata(root, folder, json!({ "requires": {} })).await;
    }

    async fn write_instruction_skill_without_metadata(
        root: &Path,
        folder: &str,
        name: &str,
        description: &str,
    ) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .await
        .unwrap();
    }

    async fn write_package_metadata(root: &Path, folder: &str, metadata: serde_json::Value) {
        let skill_dir = root.join(folder);
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(skill_dir.join("agentweave.json"), metadata.to_string())
            .await
            .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentweave-dev-skills-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: PathBuf, link: PathBuf) {
        unix_fs::symlink(&target, &link).unwrap();
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            fs::remove_dir_all(path).await.unwrap();
        }
    }
}
