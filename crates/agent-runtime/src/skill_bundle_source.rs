use super::{
    SKILL_BUNDLE_LOCK_FILE, SKILL_BUNDLE_MANIFEST_FILE, SKILL_BUNDLE_SCHEMA_VERSION,
    SkillBundleLock, SkillBundleManifest,
};
use crate::skill_package::{DescriptorSource, SkillCompatibility, SkillPackageDescriptor};
use crate::skill_source::{
    DiscoveredSkillPackage, SkillLayer, SkillSource, VerifiedPackageContent,
    canonical_relative_path,
};
use crate::skill_store::SkillStoreLimits;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_prepared_fs::open_regular_file;
use crate::skill_store_secure_fs::secure_package_snapshot;
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

#[cfg(all(test, unix))]
use std::sync::{Mutex, OnceLock};

#[cfg(all(test, unix))]
#[derive(Clone)]
pub(crate) struct BundleMetadataGate {
    path: PathBuf,
    entered: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(all(test, unix))]
impl BundleMetadataGate {
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

#[cfg(all(test, unix))]
pub(crate) fn gate_bundle_metadata_after_inspection(path: &Path) -> BundleMetadataGate {
    let gate = BundleMetadataGate {
        path: path.to_path_buf(),
        entered: Arc::new(std::sync::Barrier::new(2)),
        release: Arc::new(std::sync::Barrier::new(2)),
    };
    *metadata_gate().lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(all(test, unix))]
fn metadata_gate() -> &'static Mutex<Option<BundleMetadataGate>> {
    static GATE: OnceLock<Mutex<Option<BundleMetadataGate>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

#[cfg(all(test, unix))]
async fn checkpoint_metadata_after_inspection(path: &Path) {
    let gate = {
        let mut slot = metadata_gate().lock().unwrap();
        if slot.as_ref().is_some_and(|gate| gate.path == path) {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let entered = gate.entered.clone();
        tokio::task::spawn_blocking(move || entered.wait())
            .await
            .unwrap();
        let release = gate.release.clone();
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
    }
}

#[cfg(not(all(test, unix)))]
async fn checkpoint_metadata_after_inspection(_path: &Path) {}

#[derive(Clone, Debug)]
pub struct BundleSkillSource {
    root: PathBuf,
    root_identity: StoreRootIdentity,
    packages: Vec<DiscoveredSkillPackage>,
}

impl BundleSkillSource {
    pub async fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        let metadata = tokio::fs::symlink_metadata(&root)
            .await
            .with_context(|| format!("failed to inspect bundle root {}", root.display()))?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "bundle root must be a real directory: {}",
            root.display()
        );
        let root = tokio::fs::canonicalize(&root).await?;
        let root_identity = StoreRootIdentity::capture(root.clone())?;
        let prepared_root = PreparedStoreDirectory::open(&root_identity, Path::new(""))?;
        let manifest: SkillBundleManifest =
            read_json(&prepared_root, Path::new(SKILL_BUNDLE_MANIFEST_FILE)).await?;
        let lock: SkillBundleLock =
            read_json(&prepared_root, Path::new(SKILL_BUNDLE_LOCK_FILE)).await?;
        anyhow::ensure!(
            manifest.schema_version == SKILL_BUNDLE_SCHEMA_VERSION,
            "unsupported skill bundle manifest schema version: {}",
            manifest.schema_version
        );
        anyhow::ensure!(
            lock.schema_version == SKILL_BUNDLE_SCHEMA_VERSION,
            "unsupported skill bundle lock schema version: {}",
            lock.schema_version
        );
        validate_canonical_order(&manifest, &lock)?;
        let manifest_by_id = unique_manifest_packages(&manifest)?;
        let lock_by_id = unique_lock_packages(&lock)?;
        anyhow::ensure!(
            manifest_by_id.keys().eq(lock_by_id.keys()),
            "bundle manifest and lock package sets do not match"
        );
        for package in &manifest.packages {
            validate_package_path(package)?;
        }
        verify_top_level_entries(&root, &manifest).await?;

        let mut packages = Vec::with_capacity(manifest.packages.len());
        for manifest_package in &manifest.packages {
            let locked = lock_by_id
                .get(&manifest_package.id)
                .context("bundle lock package is missing")?;
            anyhow::ensure!(
                manifest_package.version == locked.version
                    && manifest_package.content_hash == locked.content_hash,
                "bundle manifest and lock disagree for {}",
                manifest_package.id.as_str()
            );
            let package_root = root.join(&manifest_package.path);
            let canonical_package =
                tokio::fs::canonicalize(&package_root)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to resolve bundle package {}",
                            manifest_package.id.as_str()
                        )
                    })?;
            anyhow::ensure!(
                canonical_package.parent() == Some(root.as_path()),
                "bundle package path escapes bundle root: {}",
                manifest_package.path.display()
            );
            let snapshot = secure_package_snapshot(
                &canonical_package,
                SkillStoreLimits::default().package_limits(),
            )
            .await?;
            anyhow::ensure!(
                snapshot.content_hash == locked.content_hash,
                "content hash mismatch for {}",
                manifest_package.id.as_str()
            );
            anyhow::ensure!(
                snapshot.descriptor.source == DescriptorSource::Explicit,
                "bundled package requires an explicit descriptor"
            );
            let expected = expected_descriptor(manifest_package, locked.dependencies.clone());
            let actual = canonical_descriptor(snapshot.descriptor.descriptor);
            anyhow::ensure!(
                actual == expected,
                "bundle descriptor does not match manifest and lock for {}",
                manifest_package.id.as_str()
            );
            let limits = SkillStoreLimits::default();
            packages.push(DiscoveredSkillPackage {
                layer: SkillLayer::Builtin,
                root: canonical_package,
                descriptor: expected,
                content_hash: locked.content_hash.clone(),
                warnings: Vec::new(),
                verified_content: Some(VerifiedPackageContent {
                    runtime_manifest: snapshot.runtime_manifest.map(Arc::from),
                    instructions_file: snapshot.instructions_file.map(Arc::from),
                    expected_content_hash: locked.content_hash.clone(),
                    limits,
                    execution_binding: None,
                }),
            });
        }
        packages.sort_by(|left, right| left.descriptor.id.cmp(&right.descriptor.id));
        root_identity.verify("bundle")?;
        Ok(Self {
            root,
            root_identity,
            packages,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn validate_canonical_order(
    manifest: &SkillBundleManifest,
    lock: &SkillBundleLock,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        manifest
            .packages
            .windows(2)
            .all(|items| items[0].id < items[1].id),
        "bundle manifest packages must be sorted and unique"
    );
    anyhow::ensure!(
        lock.packages
            .windows(2)
            .all(|items| items[0].id < items[1].id),
        "bundle lock packages must be sorted and unique"
    );
    for package in &manifest.packages {
        ensure_sorted_unique(&package.platforms, "platforms", package.id.as_str())?;
        ensure_sorted_unique(&package.capabilities, "capabilities", package.id.as_str())?;
        ensure_sorted_unique(&package.runtime_tools, "runtime tools", package.id.as_str())?;
        ensure_sorted_unique(&package.connectors, "connectors", package.id.as_str())?;
    }
    for package in &lock.packages {
        anyhow::ensure!(
            package
                .dependencies
                .windows(2)
                .all(|items| items[0] < items[1]),
            "bundle lock dependencies must be sorted and unique for {}",
            package.id.as_str()
        );
    }
    Ok(())
}

fn ensure_sorted_unique(values: &[String], label: &str, package_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        values.windows(2).all(|items| items[0] < items[1]),
        "bundle manifest {label} must be sorted and unique for {package_id}"
    );
    Ok(())
}

#[async_trait]
impl SkillSource for BundleSkillSource {
    fn layer(&self) -> SkillLayer {
        SkillLayer::Builtin
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        self.root_identity.verify("bundle")?;
        for package in &self.packages {
            let current = secure_package_snapshot(
                &package.root,
                SkillStoreLimits::default().package_limits(),
            )
            .await?;
            anyhow::ensure!(
                current.content_hash == package.content_hash,
                "content hash mismatch for {}",
                package.descriptor.id.as_str()
            );
        }
        self.root_identity.verify("bundle")?;
        Ok(self.packages.clone())
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(
    root: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<T> {
    root.verify()?;
    let path = root.path().join(relative);
    checkpoint_metadata_after_inspection(&path).await;
    let (file, expected_bytes, _) = open_regular_file(root, relative)
        .await
        .with_context(|| format!("failed to open bundle metadata {}", path.display()))?;
    let limits = SkillStoreLimits::default();
    anyhow::ensure!(
        expected_bytes <= limits.max_file_bytes,
        "bundle metadata exceeds {} byte limit: {}",
        limits.max_file_bytes,
        path.display()
    );
    let mut bytes = Vec::with_capacity(usize::try_from(expected_bytes)?);
    file.take(limits.max_file_bytes + 1)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("failed to read bundle metadata {}", path.display()))?;
    anyhow::ensure!(
        u64::try_from(bytes.len())? == expected_bytes,
        "bundle metadata changed while reading: {}",
        path.display()
    );
    root.verify()?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse bundle metadata {}", path.display()))
}

fn unique_manifest_packages(
    manifest: &SkillBundleManifest,
) -> anyhow::Result<BTreeMap<&crate::skill_package::SkillPackageId, &super::SkillBundlePackage>> {
    let mut by_id = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for package in &manifest.packages {
        anyhow::ensure!(
            by_id.insert(&package.id, package).is_none(),
            "duplicate package id in bundle manifest: {}",
            package.id.as_str()
        );
        anyhow::ensure!(
            paths.insert(&package.path),
            "duplicate package path in bundle manifest: {}",
            package.path.display()
        );
    }
    Ok(by_id)
}

fn unique_lock_packages(
    lock: &SkillBundleLock,
) -> anyhow::Result<BTreeMap<&crate::skill_package::SkillPackageId, &super::SkillBundleLockPackage>>
{
    let mut by_id = BTreeMap::new();
    for package in &lock.packages {
        anyhow::ensure!(
            by_id.insert(&package.id, package).is_none(),
            "duplicate package id in bundle lock: {}",
            package.id.as_str()
        );
        anyhow::ensure!(
            package.dependencies.windows(2).all(|ids| ids[0] < ids[1]),
            "bundle lock dependencies must be sorted and unique for {}",
            package.id.as_str()
        );
    }
    Ok(by_id)
}

fn validate_package_path(package: &super::SkillBundlePackage) -> anyhow::Result<()> {
    canonical_relative_path(&package.path)?;
    anyhow::ensure!(
        package.path == PathBuf::from(package.id.as_str())
            && package.path.components().count() == 1,
        "bundle package path must equal package id: {}",
        package.path.display()
    );
    Ok(())
}

async fn verify_top_level_entries(
    root: &Path,
    manifest: &SkillBundleManifest,
) -> anyhow::Result<()> {
    let mut expected = manifest
        .packages
        .iter()
        .map(|package| package.path.clone())
        .collect::<BTreeSet<_>>();
    expected.insert(PathBuf::from(SKILL_BUNDLE_MANIFEST_FILE));
    expected.insert(PathBuf::from(SKILL_BUNDLE_LOCK_FILE));
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let relative = PathBuf::from(entry.file_name());
        anyhow::ensure!(
            expected.remove(&relative),
            "bundle contains unlocked content: {}",
            relative.display()
        );
    }
    anyhow::ensure!(expected.is_empty(), "bundle is missing locked content");
    Ok(())
}

fn expected_descriptor(
    package: &super::SkillBundlePackage,
    dependencies: Vec<crate::skill_package::SkillPackageId>,
) -> SkillPackageDescriptor {
    SkillPackageDescriptor {
        schema_version: crate::skill_package::SKILL_PACKAGE_SCHEMA_VERSION,
        id: package.id.clone(),
        version: package.version.clone(),
        display_name: package.display_name.clone(),
        kind: package.kind,
        package: package.targets(),
        compatibility: SkillCompatibility {
            minimum_runtime_version: package.minimum_runtime_version.clone(),
            platforms: package.platforms.clone(),
        },
        requires: package.requirements(dependencies),
    }
}

fn canonical_descriptor(mut descriptor: SkillPackageDescriptor) -> SkillPackageDescriptor {
    sort_dedup(&mut descriptor.compatibility.platforms);
    descriptor.requires.packages.sort();
    descriptor.requires.packages.dedup();
    sort_dedup(&mut descriptor.requires.capabilities);
    sort_dedup(&mut descriptor.requires.runtime_tools);
    sort_dedup(&mut descriptor.requires.connectors);
    descriptor
}

fn sort_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}
