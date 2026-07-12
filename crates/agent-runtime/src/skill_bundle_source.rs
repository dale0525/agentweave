use super::{
    SKILL_BUNDLE_CURRENT_FILE, SKILL_BUNDLE_GENERATIONS_DIR, SKILL_BUNDLE_LOCK_FILE,
    SKILL_BUNDLE_MANIFEST_FILE, SKILL_BUNDLE_SCHEMA_VERSION, SkillBundleCurrent, SkillBundleLock,
    SkillBundleManifest,
};
use crate::skill_package::{DescriptorSource, SkillCompatibility, SkillPackageDescriptor};
use crate::skill_source::{
    BundleExecutionBinding, DiscoveredSkillPackage, SkillLayer, SkillSource,
    VerifiedPackageContent, canonical_relative_path,
};
use crate::skill_store::SkillStoreLimits;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_operations::error_is_not_found;
use crate::skill_store_prepared_fs::open_regular_file;
use crate::skill_store_secure_roots::{
    PreparedStoreDirectory, PreparedStoreUnknownKind, list_opened_child_directories,
    open_prepared_directory, opened_package_snapshot,
};
use anyhow::Context;
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

#[cfg(test)]
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

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BundleDiscoveryGate {
    generation: PathBuf,
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
impl BundleDiscoveryGate {
    pub(crate) async fn wait_entered(&self) {
        self.entered.wait().await;
    }

    pub(crate) async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
pub(crate) fn gate_bundle_discovery_after_layout(generation: &Path) -> BundleDiscoveryGate {
    let gate = BundleDiscoveryGate {
        generation: generation.to_path_buf(),
        entered: Arc::new(tokio::sync::Barrier::new(2)),
        release: Arc::new(tokio::sync::Barrier::new(2)),
    };
    *discovery_gate().lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn discovery_gate() -> &'static Mutex<Option<BundleDiscoveryGate>> {
    static GATE: OnceLock<Mutex<Option<BundleDiscoveryGate>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
async fn checkpoint_discovery_after_layout(generation: &Path) {
    let gate = {
        let mut slot = discovery_gate().lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|gate| gate.generation == generation)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        gate.entered.wait().await;
        gate.release.wait().await;
    }
}

#[cfg(not(test))]
async fn checkpoint_discovery_after_layout(_generation: &Path) {}

#[derive(Clone, Debug)]
pub struct BundleSkillSource {
    root: PathBuf,
    root_identity: StoreRootIdentity,
    prepared_root: PreparedStoreDirectory,
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
        let source = Self {
            root,
            root_identity,
            prepared_root,
        };
        source.load_current_packages().await?;
        Ok(source)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn load_current_packages(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        self.root_identity.verify("bundle")?;
        self.prepared_root.verify()?;
        let generation = match read_json::<SkillBundleCurrent>(
            &self.prepared_root,
            Path::new(SKILL_BUNDLE_CURRENT_FILE),
        )
        .await
        {
            Ok(current) => {
                anyhow::ensure!(
                    current.schema_version == SKILL_BUNDLE_SCHEMA_VERSION,
                    "unsupported skill bundle current schema version: {}",
                    current.schema_version
                );
                validate_generation_id(&current.generation)?;
                verify_generation_container(&self.prepared_root).await?;
                let relative =
                    PathBuf::from(SKILL_BUNDLE_GENERATIONS_DIR).join(&current.generation);
                open_prepared_directory(&self.root_identity, &relative)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to open current bundle generation {}",
                            current.generation
                        )
                    })?
            }
            Err(error) if error_is_not_found(&error) => {
                if has_direct_bundle_evidence(&self.prepared_root).await? {
                    self.prepared_root.clone()
                } else {
                    return Err(error).context("bundle current marker is missing");
                }
            }
            Err(error) => return Err(error),
        };
        let packages = load_generation(&generation, &self.root).await?;
        generation.verify()?;
        self.root_identity.verify("bundle")?;
        Ok(packages)
    }
}

async fn load_generation(
    generation: &PreparedStoreDirectory,
    bundle_root: &Path,
) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
    let manifest: SkillBundleManifest =
        read_json(generation, Path::new(SKILL_BUNDLE_MANIFEST_FILE)).await?;
    let lock: SkillBundleLock = read_json(generation, Path::new(SKILL_BUNDLE_LOCK_FILE)).await?;
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
    validate_dependency_closure(&lock_by_id)?;
    for package in &manifest.packages {
        validate_package_path(package)?;
    }
    let package_directories = verify_top_level_entries(generation, &manifest).await?;
    checkpoint_discovery_after_layout(generation.path()).await;

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
        let package_directory = package_directories
            .get(manifest_package.id.as_str())
            .context("bundle package directory is missing")?;
        let snapshot = opened_package_snapshot(
            package_directory,
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
            root: package_directory.path().to_path_buf(),
            descriptor: expected,
            content_hash: locked.content_hash.clone(),
            warnings: Vec::new(),
            verified_content: Some(VerifiedPackageContent {
                runtime_manifest: snapshot.runtime_manifest.map(Arc::from),
                instructions_file: snapshot.instructions_file.map(Arc::from),
                expected_content_hash: locked.content_hash.clone(),
                limits,
                execution_binding: None,
                bundle_execution_binding: Some(BundleExecutionBinding {
                    directory: package_directory.clone(),
                    bundle_root: bundle_root.to_path_buf(),
                }),
            }),
        });
    }
    packages.sort_by(|left, right| left.descriptor.id.cmp(&right.descriptor.id));
    verify_top_level_entries(generation, &manifest).await?;
    generation.verify()?;
    Ok(packages)
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
        self.load_current_packages().await
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

fn validate_dependency_closure(
    lock_by_id: &BTreeMap<&crate::skill_package::SkillPackageId, &super::SkillBundleLockPackage>,
) -> anyhow::Result<()> {
    for package in lock_by_id.values() {
        for dependency in &package.dependencies {
            anyhow::ensure!(
                lock_by_id.contains_key(dependency),
                "bundle lock dependency is missing from locked set for {}: {}",
                package.id.as_str(),
                dependency.as_str()
            );
        }
    }
    Ok(())
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
    root: &PreparedStoreDirectory,
    manifest: &SkillBundleManifest,
) -> anyhow::Result<BTreeMap<String, PreparedStoreDirectory>> {
    let mut expected = manifest
        .packages
        .iter()
        .map(|package| package.id.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let listing = list_opened_child_directories(root, 4096).await?;
    anyhow::ensure!(
        !listing.exceeded,
        "bundle contains too many top-level entries"
    );
    let mut directories = BTreeMap::new();
    for child in listing.children {
        anyhow::ensure!(
            expected.remove(&child.name),
            "bundle contains unlocked content: {}",
            child.name
        );
        directories.insert(child.name, child.directory);
    }
    let mut metadata = BTreeSet::from([
        SKILL_BUNDLE_MANIFEST_FILE.to_string(),
        SKILL_BUNDLE_LOCK_FILE.to_string(),
    ]);
    for unknown in listing.unknown {
        anyhow::ensure!(
            unknown.kind == PreparedStoreUnknownKind::RegularFile && metadata.remove(&unknown.name),
            "bundle contains unlocked content: {}",
            unknown.name
        );
    }
    anyhow::ensure!(
        expected.is_empty() && metadata.is_empty(),
        "bundle is missing locked content"
    );
    Ok(directories)
}

fn validate_generation_id(generation: &str) -> anyhow::Result<()> {
    let parsed = uuid::Uuid::parse_str(generation)
        .with_context(|| format!("invalid bundle generation id: {generation}"))?;
    anyhow::ensure!(
        parsed.to_string() == generation,
        "bundle generation id is not canonical: {generation}"
    );
    Ok(())
}

async fn verify_generation_container(root: &PreparedStoreDirectory) -> anyhow::Result<()> {
    let listing = list_opened_child_directories(root, 4096).await?;
    anyhow::ensure!(!listing.exceeded, "bundle container has too many entries");
    anyhow::ensure!(
        listing.children.len() == 1 && listing.children[0].name == SKILL_BUNDLE_GENERATIONS_DIR,
        "bundle container layout is invalid"
    );
    verify_generation_store(&listing.children[0].directory).await?;
    let mut has_current = false;
    for unknown in listing.unknown {
        if unknown.name == SKILL_BUNDLE_CURRENT_FILE
            && unknown.kind == PreparedStoreUnknownKind::RegularFile
        {
            has_current = true;
            continue;
        }
        let is_atomic_temporary = unknown.kind == PreparedStoreUnknownKind::RegularFile
            && unknown.name.starts_with(".skill-write-")
            && unknown.name.ends_with(".tmp");
        anyhow::ensure!(
            is_atomic_temporary,
            "bundle container contains unlocked content: {}",
            unknown.name
        );
    }
    anyhow::ensure!(has_current, "bundle current marker is missing");
    Ok(())
}

async fn verify_generation_store(store: &PreparedStoreDirectory) -> anyhow::Result<()> {
    let listing = list_opened_child_directories(store, 4096).await?;
    anyhow::ensure!(
        !listing.exceeded,
        "bundle generation store contains too many entries"
    );
    anyhow::ensure!(
        listing.unknown.is_empty(),
        "bundle generation store contains unlocked generation content"
    );
    for child in listing.children {
        validate_generation_id(&child.name).with_context(|| {
            format!(
                "bundle generation store contains unlocked generation: {}",
                child.name
            )
        })?;
    }
    Ok(())
}

async fn has_direct_bundle_evidence(root: &PreparedStoreDirectory) -> anyhow::Result<bool> {
    root.verify()?;
    let manifest = tokio::fs::symlink_metadata(root.path().join(SKILL_BUNDLE_MANIFEST_FILE)).await;
    let lock = tokio::fs::symlink_metadata(root.path().join(SKILL_BUNDLE_LOCK_FILE)).await;
    root.verify()?;
    Ok(manifest.is_ok() || lock.is_ok())
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
