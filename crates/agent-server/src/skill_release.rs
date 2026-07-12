use agent_runtime::{
    platform::CapabilitySet,
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    skill_package::SkillPackageId,
    skill_source::{DirectorySkillSource, DiscoveredSkillPackage, SkillLayer, SkillSource},
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillReleaseDiagnostic {
    pub package_id: Option<String>,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillReleaseReport {
    pub roots: Vec<PathBuf>,
    pub package_count: usize,
    pub errors: Vec<SkillReleaseDiagnostic>,
    pub warnings: Vec<SkillReleaseDiagnostic>,
}

impl SkillReleaseReport {
    pub fn is_ready(&self) -> bool {
        self.errors.is_empty()
    }
}

pub async fn validate_skill_roots(roots: &[PathBuf]) -> SkillReleaseReport {
    let mut canonical_roots = BTreeSet::new();
    let mut errors = Vec::new();
    for root in roots {
        match tokio::fs::canonicalize(root).await {
            Ok(path) => {
                canonical_roots.insert(path);
            }
            Err(error) => errors.push(diagnostic(
                None,
                root.clone(),
                format!("failed to resolve skill root: {error}"),
            )),
        }
    }
    let roots = canonical_roots.into_iter().collect::<Vec<_>>();
    let mut packages = Vec::new();
    for root in &roots {
        let source = DirectorySkillSource::new(SkillLayer::Builtin, root);
        match source.discover_read_only().await {
            Ok(mut discovered) => packages.append(&mut discovered),
            Err(error) => errors.push(diagnostic(
                None,
                root.clone(),
                format!("package discovery failed: {error:#}"),
            )),
        }
    }
    packages.sort_by(|left, right| {
        left.descriptor
            .id
            .cmp(&right.descriptor.id)
            .then_with(|| left.root.cmp(&right.root))
    });
    let package_count = packages.len();
    let mut warnings = legacy_warnings(&packages);
    validate_duplicate_ids(&packages, &mut errors);
    let available_ids = packages
        .iter()
        .map(|package| package.descriptor.id.clone())
        .collect::<BTreeSet<_>>();
    let runtime_tools = collect_runtime_tools(&packages, &mut errors).await;
    validate_instruction_packages(&packages, &mut errors).await;
    validate_requirements(
        &packages,
        &available_ids,
        &runtime_tools,
        &known_capabilities(),
        &mut errors,
    );
    sort_diagnostics(&mut errors);
    sort_diagnostics(&mut warnings);
    SkillReleaseReport {
        roots,
        package_count,
        errors,
        warnings,
    }
}

fn legacy_warnings(packages: &[DiscoveredSkillPackage]) -> Vec<SkillReleaseDiagnostic> {
    packages
        .iter()
        .filter(|package| !package.warnings.is_empty())
        .flat_map(|package| {
            package.warnings.iter().map(|warning| {
                diagnostic(
                    Some(package.descriptor.id.as_str()),
                    package.root.clone(),
                    warning.clone(),
                )
            })
        })
        .collect()
}

fn validate_duplicate_ids(
    packages: &[DiscoveredSkillPackage],
    errors: &mut Vec<SkillReleaseDiagnostic>,
) {
    let mut by_id = BTreeMap::<&SkillPackageId, Vec<&DiscoveredSkillPackage>>::new();
    for package in packages {
        by_id
            .entry(&package.descriptor.id)
            .or_default()
            .push(package);
    }
    for (id, owners) in by_id.into_iter().filter(|(_, owners)| owners.len() > 1) {
        for owner in owners {
            errors.push(diagnostic(
                Some(id.as_str()),
                owner.root.clone(),
                format!("duplicate package id: {}", id.as_str()),
            ));
        }
    }
}

async fn collect_runtime_tools(
    packages: &[DiscoveredSkillPackage],
    errors: &mut Vec<SkillReleaseDiagnostic>,
) -> BTreeSet<String> {
    let mut tools = BTreeSet::new();
    for package in packages {
        let has_runtime = regular_file_exists(&package.root.join("skill.json")).await;
        if has_runtime != package.descriptor.package.include_runtime {
            errors.push(package_error(
                package,
                "runtime include flag does not match skill.json",
            ));
            continue;
        }
        if !has_runtime {
            continue;
        }
        match SkillRegistry::load_development_skill(&package.root).await {
            Ok(skill) => {
                for tool in &skill.manifest().tools {
                    tools.insert(format!("{}/{}", package.descriptor.id.as_str(), tool.name));
                }
            }
            Err(error) => errors.push(package_error(
                package,
                format!("runtime manifest error: {error:#}"),
            )),
        }
    }
    tools
}

async fn validate_instruction_packages(
    packages: &[DiscoveredSkillPackage],
    errors: &mut Vec<SkillReleaseDiagnostic>,
) {
    let mut entries = Vec::new();
    for package in packages {
        let has_instructions = regular_file_exists(&package.root.join("SKILL.md")).await;
        if has_instructions != package.descriptor.package.include_instructions {
            errors.push(package_error(
                package,
                "instruction include flag does not match SKILL.md",
            ));
            continue;
        }
        if !has_instructions {
            continue;
        }
        match SkillCatalog::read_package_entry(&package.root).await {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(package_error(
                package,
                format!("instruction manifest error: {error:#}"),
            )),
        }
    }
    if let Err(error) = SkillCatalog::from_entries(entries) {
        errors.push(diagnostic(
            None,
            PathBuf::from("<catalog>"),
            format!("instruction catalog error: {error:#}"),
        ));
    }
}

fn validate_requirements(
    packages: &[DiscoveredSkillPackage],
    available_ids: &BTreeSet<SkillPackageId>,
    runtime_tools: &BTreeSet<String>,
    capabilities: &BTreeSet<String>,
    errors: &mut Vec<SkillReleaseDiagnostic>,
) {
    for package in packages {
        for dependency in &package.descriptor.requires.packages {
            if !available_ids.contains(dependency) {
                errors.push(package_error(
                    package,
                    format!("missing dependency: {}", dependency.as_str()),
                ));
            }
        }
        for capability in &package.descriptor.requires.capabilities {
            if !capabilities.contains(capability) {
                errors.push(package_error(
                    package,
                    format!("unresolved capability: {capability}"),
                ));
            }
        }
        for connector in &package.descriptor.requires.connectors {
            errors.push(package_error(
                package,
                format!("unresolved connector: {connector}"),
            ));
        }
        for tool in &package.descriptor.requires.runtime_tools {
            if !canonical_tool_identity_is_valid(tool) || !runtime_tools.contains(tool) {
                errors.push(package_error(
                    package,
                    format!("unresolved canonical runtime tool: {tool}"),
                ));
            }
        }
    }
}

fn known_capabilities() -> BTreeSet<String> {
    [
        CapabilitySet::desktop_runtime(),
        CapabilitySet::server_runtime(),
        CapabilitySet::android_mvp(),
    ]
    .into_iter()
    .flat_map(|set| set.names().to_vec())
    .collect()
}

fn canonical_tool_identity_is_valid(value: &str) -> bool {
    let Some((package, local)) = value.split_once('/') else {
        return false;
    };
    !local.contains('/')
        && SkillPackageId::parse(package).is_ok()
        && !local.is_empty()
        && local.len() <= 64
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

async fn regular_file_exists(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path)
        .await
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn package_error(
    package: &DiscoveredSkillPackage,
    message: impl Into<String>,
) -> SkillReleaseDiagnostic {
    diagnostic(
        Some(package.descriptor.id.as_str()),
        package.root.clone(),
        message,
    )
}

fn diagnostic(
    package_id: Option<&str>,
    path: PathBuf,
    message: impl Into<String>,
) -> SkillReleaseDiagnostic {
    SkillReleaseDiagnostic {
        package_id: package_id.map(str::to_string),
        path,
        message: message.into(),
    }
}

fn sort_diagnostics(diagnostics: &mut [SkillReleaseDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        left.package_id
            .cmp(&right.package_id)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.message.cmp(&right.message))
    });
}
