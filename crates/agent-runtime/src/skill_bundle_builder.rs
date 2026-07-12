use super::{
    BuildSkillBundleRequest, BuildSkillBundleResult, SKILL_BUNDLE_LOCK_FILE,
    SKILL_BUNDLE_MANIFEST_FILE, SKILL_BUNDLE_SCHEMA_VERSION, SkillBundleLock,
    SkillBundleLockPackage, SkillBundleManifest, SkillBundlePackage,
};
use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_package::DescriptorSource;
use crate::skill_resolver::{SkillResolutionInput, SkillResolver};
use crate::skill_source::{DiscoveredSkillPackage, SkillLayer};
use crate::skill_store::SkillStoreLimits;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs::copy_package_tree_into_prepared;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_secure_fs::secure_package_snapshot;
use crate::skill_store_secure_roots::reserve_opened_directory;
use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BundleInspectionGate {
    output_root: PathBuf,
    entered: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(test)]
impl BundleInspectionGate {
    pub(crate) async fn wait_entered(&self) {
        let barrier = self.entered.clone();
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
    }

    pub(crate) async fn release(&self) {
        let barrier = self.release.clone();
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
    }
}

#[cfg(test)]
pub(crate) fn gate_bundle_after_inspection(output_root: &Path) -> BundleInspectionGate {
    let gate = BundleInspectionGate {
        output_root: output_root.to_path_buf(),
        entered: Arc::new(std::sync::Barrier::new(2)),
        release: Arc::new(std::sync::Barrier::new(2)),
    };
    *inspection_gate().lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn inspection_gate() -> &'static Mutex<Option<BundleInspectionGate>> {
    static GATE: OnceLock<Mutex<Option<BundleInspectionGate>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
async fn checkpoint_after_inspection(output_root: &Path) {
    let gate = {
        let mut slot = inspection_gate().lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|gate| gate.output_root == output_root)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let entered = gate.entered.clone();
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .expect("bundle inspection gate worker failed");
        let release = gate.release.clone();
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .expect("bundle inspection gate worker failed");
    }
}

#[cfg(not(test))]
async fn checkpoint_after_inspection(_output_root: &Path) {}

struct InspectedPackage {
    source_root: PathBuf,
    descriptor: crate::skill_package::SkillPackageDescriptor,
    content_hash: String,
}

pub async fn build_skill_bundle(
    request: BuildSkillBundleRequest,
) -> anyhow::Result<BuildSkillBundleResult> {
    validate_request(&request)?;
    let source_roots = canonical_source_roots(&request).await?;
    let output_root = absolute_normalized(&request.output_root)?;
    reject_root_overlap(&source_roots, &output_root).await?;
    let packages = inspect_packages(&source_roots).await?;
    validate_resolved_package_set(&request, &packages)?;
    checkpoint_after_inspection(&output_root).await;
    let (manifest, lock) = artifact_contract(&request, &packages);
    let manifest_bytes = pretty_json(&manifest)?;
    let lock_bytes = pretty_json(&lock)?;
    let staging = prepare_staging(&output_root).await?;

    let result = async {
        let staging_identity = StoreRootIdentity::capture(staging.clone())?;
        for package in &packages {
            let relative = PathBuf::from(package.descriptor.id.as_str());
            let destination = reserve_opened_directory(&staging_identity, &relative).await?;
            copy_package_tree_into_prepared(
                &package.source_root,
                &destination,
                SkillStoreLimits::default().package_limits(),
                &StoreFaults::default(),
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            let staged = secure_package_snapshot(
                destination.path(),
                SkillStoreLimits::default().package_limits(),
            )
            .await?;
            anyhow::ensure!(
                staged.content_hash == package.content_hash,
                "source package changed during bundle copy: staged content hash mismatch for {}",
                package.descriptor.id.as_str()
            );
        }
        tokio::fs::write(staging.join(SKILL_BUNDLE_MANIFEST_FILE), &manifest_bytes).await?;
        tokio::fs::write(staging.join(SKILL_BUNDLE_LOCK_FILE), &lock_bytes).await?;
        revalidate_sources(&packages).await?;
        publish_staging(&staging, &output_root).await
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    result?;
    Ok(BuildSkillBundleResult {
        root: output_root,
        package_count: packages.len(),
        manifest_bytes,
        lock_bytes,
    })
}

fn validate_request(request: &BuildSkillBundleRequest) -> anyhow::Result<()> {
    anyhow::ensure!(
        !request.source_roots.is_empty(),
        "at least one source root is required"
    );
    anyhow::ensure!(
        !request.output_root.as_os_str().is_empty(),
        "output root must not be empty"
    );
    anyhow::ensure!(
        !request.generated_at.trim().is_empty(),
        "generatedAt must not be empty"
    );
    Ok(())
}

fn validate_resolved_package_set(
    request: &BuildSkillBundleRequest,
    packages: &[InspectedPackage],
) -> anyhow::Result<()> {
    let discovered = packages
        .iter()
        .map(|package| DiscoveredSkillPackage {
            layer: SkillLayer::Builtin,
            root: package.source_root.clone(),
            descriptor: package.descriptor.clone(),
            content_hash: package.content_hash.clone(),
            warnings: Vec::new(),
            verified_content: None,
        })
        .collect();
    let resolved = SkillResolver::resolve(SkillResolutionInput {
        packages: discovered,
        platform: request.platform,
        capabilities: platform_capabilities(request.platform),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: request.runtime_version.clone(),
    })?;
    if let Some(inactive) = resolved.inactive.first() {
        anyhow::bail!(
            "inactive package {}: {}",
            inactive.package.descriptor.id.as_str(),
            inactive.reason
        );
    }
    anyhow::ensure!(
        resolved.active.len() == packages.len(),
        "bundle package resolution omitted a package"
    );
    Ok(())
}

fn platform_capabilities(platform: PlatformId) -> CapabilitySet {
    match platform {
        PlatformId::Desktop => CapabilitySet::desktop_runtime(),
        PlatformId::Server => CapabilitySet::server_runtime(),
        PlatformId::Android => CapabilitySet::android_mvp(),
        PlatformId::Ios | PlatformId::Web => CapabilitySet::from_names(Vec::<String>::new()),
    }
}

async fn canonical_source_roots(request: &BuildSkillBundleRequest) -> anyhow::Result<Vec<PathBuf>> {
    let mut roots = BTreeSet::new();
    for root in &request.source_roots {
        let metadata = tokio::fs::symlink_metadata(root)
            .await
            .with_context(|| format!("failed to inspect source root {}", root.display()))?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "source root must be a real directory: {}",
            root.display()
        );
        roots.insert(tokio::fs::canonicalize(root).await?);
    }
    Ok(roots.into_iter().collect())
}

async fn reject_root_overlap(source_roots: &[PathBuf], output: &Path) -> anyhow::Result<()> {
    let resolved_output = canonicalize_allow_missing(output).await?;
    for source in source_roots {
        if resolved_output.starts_with(source) || source.starts_with(&resolved_output) {
            anyhow::bail!(
                "source and output roots must not overlap: {} and {}",
                source.display(),
                output.display()
            );
        }
    }
    Ok(())
}

async fn canonicalize_allow_missing(path: &Path) -> anyhow::Result<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match tokio::fs::canonicalize(existing).await {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .context("output path has no existing ancestor")?;
                missing.push(name.to_os_string());
                existing = existing
                    .parent()
                    .context("output path has no existing ancestor")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn inspect_packages(source_roots: &[PathBuf]) -> anyhow::Result<Vec<InspectedPackage>> {
    let mut packages = BTreeMap::new();
    for source_root in source_roots {
        let mut entries = tokio::fs::read_dir(source_root).await?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path());
        }
        paths.sort();
        for path in paths {
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "source root contains a non-package entry: {}",
                path.display()
            );
            let snapshot =
                secure_package_snapshot(&path, SkillStoreLimits::default().package_limits())
                    .await?;
            anyhow::ensure!(
                snapshot.descriptor.source == DescriptorSource::Explicit,
                "bundle packages require an explicit general-agent.json descriptor: {}",
                path.display()
            );
            validate_declared_content(&snapshot)?;
            let id = snapshot.descriptor.descriptor.id.clone();
            if let Some(previous) = packages.insert(
                id.clone(),
                InspectedPackage {
                    source_root: path,
                    descriptor: snapshot.descriptor.descriptor,
                    content_hash: snapshot.content_hash,
                },
            ) {
                anyhow::bail!(
                    "duplicate package id {}: {} and {}",
                    id.as_str(),
                    previous.source_root.display(),
                    packages[&id].source_root.display()
                );
            }
        }
    }
    Ok(packages.into_values().collect())
}

fn validate_declared_content(
    snapshot: &crate::skill_store_secure_snapshot::SecurePackageSnapshot,
) -> anyhow::Result<()> {
    let descriptor = &snapshot.descriptor.descriptor;
    anyhow::ensure!(
        descriptor.package.include_runtime == snapshot.runtime_manifest.is_some(),
        "runtime include flag does not match skill.json for {}",
        descriptor.id.as_str()
    );
    anyhow::ensure!(
        descriptor.package.include_instructions == snapshot.instructions_file.is_some(),
        "instruction include flag does not match SKILL.md for {}",
        descriptor.id.as_str()
    );
    Ok(())
}

fn artifact_contract(
    request: &BuildSkillBundleRequest,
    packages: &[InspectedPackage],
) -> (SkillBundleManifest, SkillBundleLock) {
    let manifest_packages = packages
        .iter()
        .map(|package| {
            let descriptor = &package.descriptor;
            let mut platforms = descriptor.compatibility.platforms.clone();
            let mut capabilities = descriptor.requires.capabilities.clone();
            let mut runtime_tools = descriptor.requires.runtime_tools.clone();
            let mut connectors = descriptor.requires.connectors.clone();
            platforms.sort();
            platforms.dedup();
            capabilities.sort();
            capabilities.dedup();
            runtime_tools.sort();
            runtime_tools.dedup();
            connectors.sort();
            connectors.dedup();
            SkillBundlePackage {
                id: descriptor.id.clone(),
                version: descriptor.version.clone(),
                display_name: descriptor.display_name.clone(),
                kind: descriptor.kind,
                path: PathBuf::from(descriptor.id.as_str()),
                content_hash: package.content_hash.clone(),
                include_instructions: descriptor.package.include_instructions,
                include_runtime: descriptor.package.include_runtime,
                minimum_runtime_version: descriptor.compatibility.minimum_runtime_version.clone(),
                platforms,
                capabilities,
                runtime_tools,
                connectors,
            }
        })
        .collect();
    let lock_packages = packages
        .iter()
        .map(|package| {
            let descriptor = &package.descriptor;
            let mut dependencies = descriptor.requires.packages.clone();
            dependencies.sort();
            dependencies.dedup();
            SkillBundleLockPackage {
                id: descriptor.id.clone(),
                version: descriptor.version.clone(),
                content_hash: package.content_hash.clone(),
                dependencies,
            }
        })
        .collect();
    (
        SkillBundleManifest {
            schema_version: SKILL_BUNDLE_SCHEMA_VERSION,
            generated_at: request.generated_at.clone(),
            packages: manifest_packages,
        },
        SkillBundleLock {
            schema_version: SKILL_BUNDLE_SCHEMA_VERSION,
            packages: lock_packages,
        },
    )
}

fn pretty_json<T: serde::Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

async fn prepare_staging(output_root: &Path) -> anyhow::Result<PathBuf> {
    let parent = output_root.parent().context("output root has no parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let staging = parent.join(format!(
        ".{}.staging-{}",
        output_root
            .file_name()
            .and_then(|name| name.to_str())
            .context("output root name must be UTF-8")?,
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir(&staging).await?;
    Ok(staging)
}

async fn revalidate_sources(packages: &[InspectedPackage]) -> anyhow::Result<()> {
    for package in packages {
        let current = secure_package_snapshot(
            &package.source_root,
            SkillStoreLimits::default().package_limits(),
        )
        .await?;
        anyhow::ensure!(
            current.content_hash == package.content_hash,
            "source package changed during bundle construction: {}",
            package.descriptor.id.as_str()
        );
    }
    Ok(())
}

async fn publish_staging(staging: &Path, output: &Path) -> anyhow::Result<()> {
    if tokio::fs::symlink_metadata(output).await.is_err() {
        return tokio::fs::rename(staging, output).await.map_err(Into::into);
    }
    let metadata = tokio::fs::symlink_metadata(output).await?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "output root must be a real directory: {}",
        output.display()
    );
    let backup = output.with_file_name(format!(
        ".{}.previous-{}",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bundle"),
        uuid::Uuid::new_v4()
    ));
    tokio::fs::rename(output, &backup).await?;
    if let Err(error) = tokio::fs::rename(staging, output).await {
        let restore = tokio::fs::rename(&backup, output).await;
        return match restore {
            Ok(()) => Err(error.into()),
            Err(restore) => Err(anyhow::anyhow!(
                "bundle publish failed: {error}; previous output restore failed: {restore}"
            )),
        };
    }
    let _ = tokio::fs::remove_dir_all(backup).await;
    Ok(())
}

fn absolute_normalized(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(Path::new("/")),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                anyhow::ensure!(normalized.pop(), "output root escapes filesystem root");
            }
            std::path::Component::Normal(name) => normalized.push(name),
        }
    }
    Ok(normalized)
}
