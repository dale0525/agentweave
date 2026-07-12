use super::{
    BuildSkillBundleRequest, BuildSkillBundleResult, SKILL_BUNDLE_CURRENT_FILE,
    SKILL_BUNDLE_GENERATIONS_DIR, SKILL_BUNDLE_LOCK_FILE, SKILL_BUNDLE_MANIFEST_FILE,
    SKILL_BUNDLE_SCHEMA_VERSION, SkillBundleCurrent, SkillBundleLock, SkillBundleLockPackage,
    SkillBundleManifest, SkillBundlePackage,
};
use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_package::DescriptorSource;
use crate::skill_resolver::{SkillResolutionInput, SkillResolver};
use crate::skill_source::{DiscoveredSkillPackage, SkillLayer};
use crate::skill_store::SkillStoreLimits;
use crate::skill_store_atomic_write::atomic_replace_replaceable_file;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs::{
    copy_package_tree_into_prepared, make_tree_readonly, make_tree_writable,
};
use crate::skill_store_fs_types::AtomicReplaceCommitState;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_prepared_fs::{
    create_regular_file, open_regular_file, set_readonly, set_writable,
};
use crate::skill_store_secure_fs::secure_package_snapshot;
use crate::skill_store_secure_roots::{
    PreparedStoreDirectory, PreparedStoreUnknownKind, ensure_opened_child_directory,
    list_opened_child_directories, open_prepared_directory, opened_package_snapshot,
    remove_opened_tree, reserve_opened_directory,
};
use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BundlePublishGate {
    generation: Arc<Mutex<Option<PathBuf>>>,
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
impl BundlePublishGate {
    pub(crate) async fn wait_entered(&self) -> PathBuf {
        self.entered.wait().await;
        self.generation.lock().unwrap().clone().unwrap()
    }

    pub(crate) async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
pub(crate) fn gate_bundle_before_publish(output_root: &Path) -> BundlePublishGate {
    let gate = BundlePublishGate {
        generation: Arc::new(Mutex::new(None)),
        entered: Arc::new(tokio::sync::Barrier::new(2)),
        release: Arc::new(tokio::sync::Barrier::new(2)),
    };
    publish_gates()
        .lock()
        .unwrap()
        .insert(output_root.to_path_buf(), gate.clone());
    gate
}

#[cfg(test)]
fn publish_gates() -> &'static Mutex<BTreeMap<PathBuf, BundlePublishGate>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, BundlePublishGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
async fn checkpoint_before_publish(output_root: &Path, generation: &Path) {
    let gate = publish_gates().lock().unwrap().remove(output_root);
    if let Some(gate) = gate {
        *gate.generation.lock().unwrap() = Some(generation.to_path_buf());
        gate.entered.wait().await;
        gate.release.wait().await;
    }
}

#[cfg(not(test))]
async fn checkpoint_before_publish(_output_root: &Path, _generation: &Path) {}

struct InspectedPackage {
    source_root: PathBuf,
    descriptor: crate::skill_package::SkillPackageDescriptor,
    content_hash: String,
}

pub async fn build_skill_bundle(
    request: BuildSkillBundleRequest,
) -> anyhow::Result<BuildSkillBundleResult> {
    build_skill_bundle_inner(request, StoreFaults::default()).await
}

#[cfg(test)]
pub(crate) async fn build_skill_bundle_with_faults(
    request: BuildSkillBundleRequest,
    faults: StoreFaults,
) -> anyhow::Result<BuildSkillBundleResult> {
    build_skill_bundle_inner(request, faults).await
}

async fn build_skill_bundle_inner(
    request: BuildSkillBundleRequest,
    faults: StoreFaults,
) -> anyhow::Result<BuildSkillBundleResult> {
    validate_request(&request)?;
    let source_roots = canonical_source_roots(&request).await?;
    let output_root = absolute_normalized(&request.output_root)?;
    let packages = inspect_packages(&source_roots).await?;
    validate_resolved_package_set(&request, &packages)?;
    checkpoint_after_inspection(&output_root).await;
    let (manifest, lock) = artifact_contract(&request, &packages);
    let manifest_bytes = pretty_json(&manifest)?;
    let lock_bytes = pretty_json(&lock)?;
    let output = prepare_output(&source_roots, &output_root).await?;
    let generation_id = uuid::Uuid::new_v4().to_string();
    let generation_relative = PathBuf::from(SKILL_BUNDLE_GENERATIONS_DIR).join(&generation_id);
    let generation = reserve_opened_directory(&output.identity, &generation_relative).await?;
    let generation_identity = StoreRootIdentity::capture(generation.path().to_path_buf())?;
    let mut published = false;
    let mut frozen = false;

    let result = async {
        let mut staged_packages = BTreeMap::new();
        for package in &packages {
            let relative = PathBuf::from(package.descriptor.id.as_str());
            let destination = reserve_opened_directory(&generation_identity, &relative).await?;
            copy_package_tree_into_prepared(
                &package.source_root,
                &destination,
                SkillStoreLimits::default().package_limits(),
                &faults,
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            let staged =
                opened_package_snapshot(&destination, SkillStoreLimits::default().package_limits())
                    .await?;
            anyhow::ensure!(
                staged.content_hash == package.content_hash,
                "source package changed during bundle copy: staged content hash mismatch for {}",
                package.descriptor.id.as_str()
            );
            staged_packages.insert(package.descriptor.id.as_str().to_string(), destination);
        }
        let manifest_identity =
            write_generation_metadata(&generation, SKILL_BUNDLE_MANIFEST_FILE, &manifest_bytes)
                .await?;
        let lock_identity =
            write_generation_metadata(&generation, SKILL_BUNDLE_LOCK_FILE, &lock_bytes).await?;
        let metadata = StagedGenerationMetadata {
            manifest_bytes: &manifest_bytes,
            manifest_identity: &manifest_identity,
            lock_bytes: &lock_bytes,
            lock_identity: &lock_identity,
        };
        revalidate_sources(&packages).await?;
        generation.verify()?;
        generation_identity.verify("bundle generation")?;
        checkpoint_before_publish(&output_root, generation.path()).await;
        faults.check(StoreFaultPoint::BundleBeforePublish)?;
        validate_staged_generation(
            &generation,
            &generation_identity,
            &staged_packages,
            &manifest,
            &metadata,
        )
        .await?;
        frozen = true;
        freeze_staged_generation(&generation, &staged_packages).await?;
        validate_staged_generation(
            &generation,
            &generation_identity,
            &staged_packages,
            &manifest,
            &metadata,
        )
        .await?;
        let publication =
            publish_generation(&output.root, &generation_id, &faults, &mut published).await;
        let thaw = thaw_staged_generation(&generation).await;
        frozen = thaw.is_err();
        match (publication, thaw) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context(
                "published bundle generation remained safely read-only after publication",
            )),
            (Err(error), Err(thaw)) => Err(error.context(format!(
                "bundle generation remained safely read-only after publication failure: {thaw:#}"
            ))),
        }
    }
    .await;

    if let Err(error) = result {
        if published {
            return Err(error);
        }
        if frozen {
            thaw_staged_generation(&generation).await?;
        }
        return match remove_opened_tree(&generation).await {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!(
                "unpublished bundle generation cleanup failed safely: {cleanup:#}"
            ))),
        };
    }
    Ok(BuildSkillBundleResult {
        root: output_root,
        package_count: packages.len(),
        manifest_bytes,
        lock_bytes,
    })
}

struct StagedGenerationMetadata<'a> {
    manifest_bytes: &'a [u8],
    manifest_identity: &'a same_file::Handle,
    lock_bytes: &'a [u8],
    lock_identity: &'a same_file::Handle,
}

async fn validate_staged_generation(
    generation: &PreparedStoreDirectory,
    generation_identity: &StoreRootIdentity,
    staged_packages: &BTreeMap<String, PreparedStoreDirectory>,
    manifest: &SkillBundleManifest,
    metadata: &StagedGenerationMetadata<'_>,
) -> anyhow::Result<()> {
    generation.verify()?;
    generation_identity.verify("bundle generation")?;
    verify_staged_layout(generation, staged_packages).await?;
    verify_staged_metadata(
        generation,
        SKILL_BUNDLE_MANIFEST_FILE,
        metadata.manifest_bytes,
        metadata.manifest_identity,
    )
    .await?;
    verify_staged_metadata(
        generation,
        SKILL_BUNDLE_LOCK_FILE,
        metadata.lock_bytes,
        metadata.lock_identity,
    )
    .await?;
    let expected_hashes = manifest
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package.content_hash.as_str()))
        .collect::<BTreeMap<_, _>>();
    for (id, directory) in staged_packages {
        directory.verify()?;
        let snapshot =
            opened_package_snapshot(directory, SkillStoreLimits::default().package_limits())
                .await?;
        anyhow::ensure!(
            expected_hashes.get(id.as_str()).copied() == Some(snapshot.content_hash.as_str()),
            "staged bundle package content changed before publication: {id}"
        );
        directory.verify()?;
    }
    verify_staged_layout(generation, staged_packages).await?;
    generation.verify()?;
    generation_identity.verify("bundle generation")
}

async fn verify_staged_layout(
    generation: &PreparedStoreDirectory,
    staged_packages: &BTreeMap<String, PreparedStoreDirectory>,
) -> anyhow::Result<()> {
    let listing = list_opened_child_directories(generation, 4096).await?;
    anyhow::ensure!(!listing.exceeded, "staged bundle contains too many entries");
    let actual_packages = listing
        .children
        .iter()
        .map(|child| child.name.as_str())
        .collect::<BTreeSet<_>>();
    let expected_packages = staged_packages
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        actual_packages == expected_packages,
        "staged bundle top-level package layout changed before publication"
    );
    let actual_metadata = listing
        .unknown
        .iter()
        .filter(|entry| entry.kind == PreparedStoreUnknownKind::RegularFile)
        .map(|entry| entry.name.as_str())
        .collect::<BTreeSet<_>>();
    let expected_metadata = BTreeSet::from([SKILL_BUNDLE_MANIFEST_FILE, SKILL_BUNDLE_LOCK_FILE]);
    anyhow::ensure!(
        listing.unknown.len() == expected_metadata.len() && actual_metadata == expected_metadata,
        "staged bundle metadata layout changed before publication"
    );
    for directory in staged_packages.values() {
        directory.verify()?;
    }
    Ok(())
}

async fn verify_staged_metadata(
    generation: &PreparedStoreDirectory,
    name: &str,
    expected: &[u8],
    expected_identity: &same_file::Handle,
) -> anyhow::Result<()> {
    let (file, expected_bytes, _) = open_regular_file(generation, Path::new(name)).await?;
    let actual_identity = same_file::Handle::from_file(file.try_clone().await?.into_std().await)?;
    anyhow::ensure!(
        actual_identity == *expected_identity,
        "staged bundle metadata identity changed before publication: {name}"
    );
    anyhow::ensure!(
        expected_bytes == u64::try_from(expected.len())?,
        "staged bundle metadata changed before publication: {name}"
    );
    let mut actual = Vec::with_capacity(expected.len());
    file.take(expected_bytes + 1)
        .read_to_end(&mut actual)
        .await?;
    anyhow::ensure!(
        actual == expected,
        "staged bundle metadata changed before publication: {name}"
    );
    generation.verify()
}

async fn freeze_staged_generation(
    generation: &PreparedStoreDirectory,
    staged_packages: &BTreeMap<String, PreparedStoreDirectory>,
) -> anyhow::Result<()> {
    for directory in staged_packages.values() {
        make_tree_readonly(directory, SkillStoreLimits::default().package_limits()).await?;
    }
    set_readonly(
        generation,
        Some(Path::new(SKILL_BUNDLE_MANIFEST_FILE)),
        false,
    )
    .await?;
    set_readonly(generation, Some(Path::new(SKILL_BUNDLE_LOCK_FILE)), false).await?;
    set_readonly(generation, None, true).await
}

async fn thaw_staged_generation(generation: &PreparedStoreDirectory) -> anyhow::Result<()> {
    set_writable(generation, None, true).await?;
    let listing = list_opened_child_directories(generation, 4096).await?;
    for child in listing.children {
        make_tree_writable(
            &child.directory,
            SkillStoreLimits::default().package_limits(),
        )
        .await?;
    }
    for name in [SKILL_BUNDLE_MANIFEST_FILE, SKILL_BUNDLE_LOCK_FILE] {
        set_writable(generation, Some(Path::new(name)), false).await?;
    }
    Ok(())
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

struct PreparedBundleOutput {
    root: PreparedStoreDirectory,
    identity: StoreRootIdentity,
}

async fn prepare_output(
    source_roots: &[PathBuf],
    output_root: &Path,
) -> anyhow::Result<PreparedBundleOutput> {
    let parent = output_root.parent().context("output root has no parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let parent = tokio::fs::canonicalize(parent).await?;
    let parent_identity = StoreRootIdentity::capture(parent.clone())?;
    let name = output_root
        .file_name()
        .context("output root has no file name")?;
    let relative = PathBuf::from(name);
    let bound_output = parent.join(&relative);
    reject_root_overlap(source_roots, &bound_output).await?;
    let root = match tokio::fs::symlink_metadata(&bound_output).await {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "output root must be a real directory: {}",
                bound_output.display()
            );
            open_prepared_directory(&parent_identity, &relative).await?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            reserve_opened_directory(&parent_identity, &relative).await?
        }
        Err(error) => return Err(error.into()),
    };
    let identity = StoreRootIdentity::capture(root.path().to_path_buf())?;
    verify_existing_output(&root).await?;
    ensure_opened_child_directory(&root, Path::new(SKILL_BUNDLE_GENERATIONS_DIR)).await?;
    root.verify()?;
    parent_identity.verify("bundle output parent")?;
    Ok(PreparedBundleOutput { root, identity })
}

async fn verify_existing_output(root: &PreparedStoreDirectory) -> anyhow::Result<()> {
    let listing = list_opened_child_directories(root, 4096).await?;
    anyhow::ensure!(
        !listing.exceeded,
        "existing bundle output contains too many entries"
    );
    if listing.observed == 0 {
        return Ok(());
    }
    anyhow::ensure!(
        listing.children.len() == 1 && listing.children[0].name == SKILL_BUNDLE_GENERATIONS_DIR,
        "existing bundle output is not a complete generation container"
    );
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
            "existing bundle output contains unlocked content: {}",
            unknown.name
        );
    }
    anyhow::ensure!(
        has_current,
        "existing bundle output has no current generation"
    );
    Ok(())
}

async fn write_generation_metadata(
    generation: &PreparedStoreDirectory,
    name: &str,
    bytes: &[u8],
) -> anyhow::Result<same_file::Handle> {
    let mut file = create_regular_file(generation, Path::new(name), 0o644).await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    let identity = same_file::Handle::from_file(file.try_clone().await?.into_std().await)?;
    generation.verify()?;
    Ok(identity)
}

async fn publish_generation(
    output: &PreparedStoreDirectory,
    generation: &str,
    faults: &StoreFaults,
    published: &mut bool,
) -> anyhow::Result<()> {
    let bytes = pretty_json(&SkillBundleCurrent {
        schema_version: SKILL_BUNDLE_SCHEMA_VERSION,
        generation: generation.to_string(),
    })?;
    match atomic_replace_replaceable_file(
        output,
        Path::new(SKILL_BUNDLE_CURRENT_FILE),
        &bytes,
        0o644,
        faults,
    )
    .await
    {
        Ok(()) => {
            *published = true;
            Ok(())
        }
        Err(failure) => {
            *published = failure.state == AtomicReplaceCommitState::Committed;
            Err(failure.error)
        }
    }
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
