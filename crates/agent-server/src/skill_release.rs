use agent_runtime::{
    platform::CapabilitySet,
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    skill_migration::{LegacySkillMigrationDiagnostic, diagnostics_from_packages},
    skill_package::SkillPackageId,
    skill_source::{DirectorySkillSource, DiscoveredSkillPackage, SkillLayer},
};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ReleaseValidationGate {
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
impl ReleaseValidationGate {
    pub(crate) async fn wait_entered(&self) {
        wait_release_gate(&self.entered, "release validation entry").await;
    }

    pub(crate) async fn release(&self) {
        wait_release_gate(&self.release, "release validation release").await;
    }
}

#[cfg(test)]
pub(crate) fn gate_release_validation_after_discovery(path: &Path) -> ReleaseValidationGate {
    let gate = ReleaseValidationGate {
        entered: Arc::new(tokio::sync::Barrier::new(2)),
        release: Arc::new(tokio::sync::Barrier::new(2)),
    };
    release_validation_gates()
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), gate.clone());
    gate
}

#[cfg(test)]
fn release_validation_gates() -> &'static Mutex<BTreeMap<PathBuf, ReleaseValidationGate>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, ReleaseValidationGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

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
    pub legacy_migrations: Vec<LegacySkillMigrationDiagnostic>,
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
        match source.discover_release().await {
            Ok(mut discovered) => {
                packages.append(&mut discovered.packages);
                errors.extend(
                    discovered
                        .issues
                        .into_iter()
                        .map(|issue| SkillReleaseDiagnostic {
                            package_id: issue.package_id,
                            path: issue.path,
                            message: issue.message,
                        }),
                );
            }
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
    checkpoint_release_validation_after_discovery(&packages).await;
    let package_count = packages.len();
    let legacy_migrations = diagnostics_from_packages(packages.clone());
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
        legacy_migrations,
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
        let Some(verified) = package.verified_content.as_ref() else {
            errors.push(package_error(
                package,
                "release package is missing its secure discovery snapshot",
            ));
            continue;
        };
        let has_runtime = verified.runtime_manifest.is_some();
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
        let bytes = verified
            .runtime_manifest
            .as_deref()
            .expect("runtime presence checked above");
        match SkillRegistry::load_verified_release_skill(
            package.root.clone(),
            bytes,
            verified.file_paths.as_ref(),
        ) {
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
        let Some(verified) = package.verified_content.as_ref() else {
            errors.push(package_error(
                package,
                "release package is missing its secure discovery snapshot",
            ));
            continue;
        };
        let has_instructions = verified.instructions_file.is_some();
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
        let bytes = verified
            .instructions_file
            .as_deref()
            .expect("instruction presence checked above");
        match SkillCatalog::read_verified_package_entry(PathBuf::from("SKILL.md"), bytes) {
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

#[cfg(test)]
async fn checkpoint_release_validation_after_discovery(packages: &[DiscoveredSkillPackage]) {
    let gate = {
        let mut gates = release_validation_gates().lock().unwrap();
        packages
            .iter()
            .find_map(|package| gates.remove(&package.root))
    };
    if let Some(gate) = gate {
        wait_release_gate(&gate.entered, "release validation checkpoint entry").await;
        wait_release_gate(&gate.release, "release validation checkpoint release").await;
    }
}

#[cfg(not(test))]
async fn checkpoint_release_validation_after_discovery(_packages: &[DiscoveredSkillPackage]) {}

#[cfg(test)]
async fn wait_release_gate(barrier: &tokio::sync::Barrier, label: &str) {
    tokio::time::timeout(Duration::from_secs(10), barrier.wait())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
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
