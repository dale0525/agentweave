use crate::skill_package::SkillPackageDescriptor;
use crate::skill_state::{
    SkillLayerRecord, SkillRevisionRecord, SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use anyhow::Context;
use async_trait::async_trait;
use icu_casemap::CaseMapper;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;
use unicode_normalization::UnicodeNormalization;

const TREE_HASH_DOMAIN: &[u8] = b"general-agent.skill-package-tree";
const TREE_HASH_VERSION: u32 = 1;
const TREE_HASH_FILE_ENTRY: u8 = 1;
const TREE_HASH_READ_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillLayer {
    Builtin,
    Managed,
    Session,
}

#[derive(Clone, Debug)]
pub struct DiscoveredSkillPackage {
    pub layer: SkillLayer,
    pub root: PathBuf,
    pub descriptor: SkillPackageDescriptor,
    pub content_hash: String,
    pub warnings: Vec<String>,
}

#[async_trait]
pub trait SkillSource: Send + Sync {
    fn layer(&self) -> SkillLayer;
    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>>;
}

pub struct DirectorySkillSource {
    layer: SkillLayer,
    root: PathBuf,
}

impl DirectorySkillSource {
    pub fn new(layer: SkillLayer, root: impl Into<PathBuf>) -> Self {
        Self {
            layer,
            root: root.into(),
        }
    }
}

#[async_trait]
impl SkillSource for DirectorySkillSource {
    fn layer(&self) -> SkillLayer {
        self.layer
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        let canonical_source_root = tokio::fs::canonicalize(&self.root)
            .await
            .with_context(|| format!("failed to resolve skill source {}", self.root.display()))?;
        let mut roots = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.root)
            .await
            .with_context(|| format!("failed to read skill source {}", self.root.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                roots.push(entry.path());
                continue;
            }
            if !file_type.is_symlink() {
                continue;
            }
            let Ok(target) = tokio::fs::canonicalize(entry.path()).await else {
                continue;
            };
            if !target.starts_with(&canonical_source_root) {
                continue;
            }
            if tokio::fs::metadata(&target)
                .await
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
            {
                roots.push(target);
            }
        }
        roots.sort();

        let mut packages = Vec::with_capacity(roots.len());
        let mut seen = BTreeMap::new();
        for root in roots {
            let loaded = SkillPackageDescriptor::load(&root).await?;
            loaded.descriptor.validate()?;
            if let Some(previous_root) =
                seen.insert(loaded.descriptor.id.clone(), loaded.root.clone())
            {
                anyhow::bail!(
                    "duplicate package id {} in {:?} source: {} and {}",
                    loaded.descriptor.id.as_str(),
                    self.layer,
                    previous_root.display(),
                    loaded.root.display()
                );
            }
            packages.push(DiscoveredSkillPackage {
                layer: self.layer,
                content_hash: hash_package_tree(&loaded.root).await?,
                root: loaded.root,
                descriptor: loaded.descriptor,
                warnings: loaded.warnings,
            });
        }
        packages.sort_by(|left, right| {
            left.descriptor
                .id
                .cmp(&right.descriptor.id)
                .then_with(|| left.root.cmp(&right.root))
        });
        Ok(packages)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSkillIssue {
    pub package_id: String,
    pub revision_id: String,
    pub reason: String,
    pub quarantine_error: Option<String>,
}

#[derive(Clone)]
pub struct ManagedSkillSource {
    paths: SkillStorePaths,
    state: SkillStateStore,
    store: SkillRevisionStore,
    issues: Arc<RwLock<Vec<ManagedSkillIssue>>>,
}

impl ManagedSkillSource {
    pub fn new(paths: SkillStorePaths, state: SkillStateStore) -> Self {
        Self {
            store: SkillRevisionStore::new(paths.clone(), state.clone()),
            paths,
            state,
            issues: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn issues(&self) -> Vec<ManagedSkillIssue> {
        self.issues
            .read()
            .expect("managed skill issue lock poisoned")
            .clone()
    }

    async fn validate_revision(
        &self,
        installation_package: &crate::skill_package::SkillPackageId,
        source_layer: SkillLayerRecord,
        revision_id: &str,
        canonical_managed_root: &Path,
    ) -> anyhow::Result<DiscoveredSkillPackage> {
        if source_layer != SkillLayerRecord::Managed {
            anyhow::bail!(
                "active managed revision installation has non-managed source layer: {}",
                source_layer.as_str()
            );
        }
        let revision = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("active skill revision not found: {revision_id}"))?;
        validate_managed_record(&revision, installation_package, &self.paths.managed)?;
        let stored_path = PathBuf::from(&revision.storage_path);
        let metadata = tokio::fs::symlink_metadata(&stored_path)
            .await
            .with_context(|| {
                format!(
                    "failed to inspect managed revision {}",
                    stored_path.display()
                )
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!(
                "managed revision root must be a real directory: {}",
                stored_path.display()
            );
        }
        let canonical_path = tokio::fs::canonicalize(&stored_path)
            .await
            .with_context(|| {
                format!(
                    "failed to canonicalize managed revision {}",
                    stored_path.display()
                )
            })?;
        if !canonical_path.starts_with(canonical_managed_root) {
            anyhow::bail!(
                "managed revision escapes managed root: {}",
                stored_path.display()
            );
        }
        let loaded = SkillPackageDescriptor::load(&stored_path).await?;
        loaded.descriptor.validate()?;
        if loaded.descriptor.id != revision.package_id {
            anyhow::bail!(
                "managed descriptor package {} does not match revision package {}",
                loaded.descriptor.id.as_str(),
                revision.package_id.as_str()
            );
        }
        let descriptor_json = serde_json::to_value(&loaded.descriptor)?;
        if descriptor_json != revision.descriptor_json {
            anyhow::bail!(
                "managed descriptor metadata mismatch for revision {}",
                revision.revision_id
            );
        }
        if loaded.descriptor.version.to_string() != revision.version {
            anyhow::bail!(
                "managed descriptor version does not match revision {}",
                revision.revision_id
            );
        }
        let content_hash = hash_package_tree(&stored_path).await?;
        if content_hash != revision.content_hash {
            anyhow::bail!(
                "managed content hash mismatch for revision {}",
                revision.revision_id
            );
        }
        Ok(DiscoveredSkillPackage {
            layer: SkillLayer::Managed,
            root: stored_path,
            descriptor: loaded.descriptor,
            content_hash,
            warnings: loaded.warnings,
        })
    }
}

#[async_trait]
impl SkillSource for ManagedSkillSource {
    fn layer(&self) -> SkillLayer {
        SkillLayer::Managed
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        let root_metadata = tokio::fs::symlink_metadata(&self.paths.managed)
            .await
            .with_context(|| {
                format!(
                    "failed to inspect managed skill root {}",
                    self.paths.managed.display()
                )
            })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            anyhow::bail!(
                "managed skill root must be a real directory: {}",
                self.paths.managed.display()
            );
        }
        let canonical_managed_root = tokio::fs::canonicalize(&self.paths.managed).await?;
        let installations = self.state.list_active_installations().await?;
        let mut discovered = Vec::new();
        let mut issues = Vec::new();
        for installation in installations {
            let revision_id = installation
                .active_revision_id
                .as_deref()
                .expect("active installation invariant validated by state rows");
            match self
                .validate_revision(
                    &installation.package_id,
                    installation.source_layer,
                    revision_id,
                    &canonical_managed_root,
                )
                .await
            {
                Ok(package) => discovered.push(package),
                Err(error) => {
                    let reason = format!("{error:#}");
                    let quarantine_error = self
                        .store
                        .quarantine_revision(revision_id, &reason)
                        .await
                        .err()
                        .map(|error| format!("{error:#}"));
                    issues.push(ManagedSkillIssue {
                        package_id: installation.package_id.as_str().to_string(),
                        revision_id: revision_id.to_string(),
                        reason,
                        quarantine_error,
                    });
                }
            }
        }
        discovered.sort_by(|left, right| {
            left.descriptor
                .id
                .cmp(&right.descriptor.id)
                .then_with(|| left.root.cmp(&right.root))
        });
        issues.sort_by(|left, right| {
            left.package_id
                .cmp(&right.package_id)
                .then_with(|| left.revision_id.cmp(&right.revision_id))
        });
        *self
            .issues
            .write()
            .expect("managed skill issue lock poisoned") = issues;
        Ok(discovered)
    }
}

fn validate_managed_record(
    revision: &SkillRevisionRecord,
    installation_package: &crate::skill_package::SkillPackageId,
    managed_root: &Path,
) -> anyhow::Result<()> {
    if revision.status != SkillRevisionStatus::Managed {
        anyhow::bail!("active revision {} is not managed", revision.revision_id);
    }
    if &revision.package_id != installation_package {
        anyhow::bail!(
            "active revision {} belongs to {}, not {}",
            revision.revision_id,
            revision.package_id.as_str(),
            installation_package.as_str()
        );
    }
    let expected = managed_root
        .join(revision.package_id.as_str())
        .join("revisions")
        .join(&revision.revision_id);
    if Path::new(&revision.storage_path) != expected {
        anyhow::bail!(
            "managed storage path mismatch: expected {}, found {}",
            expected.display(),
            revision.storage_path
        );
    }
    Ok(())
}

pub async fn hash_package_tree(root: &Path) -> anyhow::Result<String> {
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to inspect skill package root {}", root.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("skill package root cannot be a symlink: {}", root.display());
    }
    if !metadata.is_dir() {
        anyhow::bail!("skill package root must be a directory: {}", root.display());
    }

    let mut files = collect_relative_files(root).await?;
    files.sort_by(|left, right| left.canonical.cmp(&right.canonical));

    let mut hasher = Sha256::new();
    hasher.update(TREE_HASH_DOMAIN);
    hasher.update(TREE_HASH_VERSION.to_be_bytes());
    for file in files {
        hash_file_entry(root, &file, &mut hasher).await?;
    }
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Debug)]
struct CanonicalPackageFile {
    relative: PathBuf,
    canonical: Vec<u8>,
}

async fn collect_relative_files(root: &Path) -> anyhow::Result<Vec<CanonicalPackageFile>> {
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    let mut portable_paths = BTreeMap::new();
    while let Some(relative_directory) = stack.pop() {
        let directory = root.join(&relative_directory);
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read package directory {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let relative = relative_directory.join(entry.file_name());
            let path = root.join(&relative);
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .with_context(|| format!("failed to inspect package path {}", path.display()))?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                anyhow::bail!("skill package cannot contain symlinks: {}", path.display());
            }
            let identity = portable_path_identity(&relative)?;
            register_portable_path(&mut portable_paths, &relative, &identity.collision_key)?;
            if kind.is_dir() {
                stack.push(relative);
                continue;
            }
            if kind.is_file() {
                files.push(CanonicalPackageFile {
                    canonical: identity.canonical,
                    relative,
                });
                continue;
            }
            anyhow::bail!(
                "skill package cannot contain special files: {}",
                path.display()
            );
        }
    }
    Ok(files)
}

async fn hash_file_entry(
    root: &Path,
    file: &CanonicalPackageFile,
    hasher: &mut Sha256,
) -> anyhow::Result<()> {
    let path = root.join(&file.relative);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .with_context(|| format!("failed to inspect package file {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("skill package cannot contain symlinks: {}", path.display());
    }
    if !metadata.is_file() {
        anyhow::bail!(
            "skill package path must remain a regular file: {}",
            path.display()
        );
    }

    let path_length = u64::try_from(file.canonical.len())
        .context("canonical package path is too long to hash")?;
    let content_length = metadata.len();
    hasher.update([TREE_HASH_FILE_ENTRY]);
    hasher.update(path_length.to_be_bytes());
    hasher.update(&file.canonical);
    hasher.update(content_length.to_be_bytes());

    let mut source = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("failed to open package file {}", path.display()))?;
    let mut buffer = vec![0; TREE_HASH_READ_BUFFER_SIZE];
    let mut bytes_read = 0_u64;
    loop {
        let count = source
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read package file {}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read
            .checked_add(u64::try_from(count)?)
            .context("package file length overflowed while hashing")?;
        hasher.update(&buffer[..count]);
    }
    if bytes_read != content_length {
        anyhow::bail!(
            "package file changed while hashing: {} (expected {content_length} bytes, read {bytes_read})",
            path.display()
        );
    }
    Ok(())
}

#[derive(Debug)]
struct PortablePathIdentity {
    canonical: Vec<u8>,
    collision_key: Vec<u8>,
}

pub(crate) fn canonical_relative_path(relative: &Path) -> anyhow::Result<Vec<u8>> {
    Ok(portable_path_identity(relative)?.canonical)
}

pub(crate) fn portable_collision_key(relative: &Path) -> anyhow::Result<Vec<u8>> {
    Ok(portable_path_identity(relative)?.collision_key)
}

fn portable_path_identity(relative: &Path) -> anyhow::Result<PortablePathIdentity> {
    let mut canonical = String::new();
    let mut collision_key = String::new();
    let case_mapper = CaseMapper::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            anyhow::bail!(
                "package paths must contain only relative normal components: {}",
                relative.display()
            );
        };
        let component = component.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "package path components must be valid UTF-8: {}",
                relative.display()
            )
        })?;
        let component = component.nfc().collect::<String>();
        validate_portable_component(&component, relative)?;
        if !canonical.is_empty() {
            canonical.push('/');
            collision_key.push('/');
        }
        canonical.push_str(&component);
        let folded = case_mapper
            .fold_string(&component)
            .as_ref()
            .nfc()
            .collect::<String>();
        collision_key.push_str(&folded);
    }
    if canonical.is_empty() {
        anyhow::bail!("package file path cannot be empty");
    }
    Ok(PortablePathIdentity {
        canonical: canonical.into_bytes(),
        collision_key: collision_key.into_bytes(),
    })
}

pub(crate) fn register_portable_path(
    portable_paths: &mut BTreeMap<Vec<u8>, PathBuf>,
    relative: &Path,
    collision_key: &[u8],
) -> anyhow::Result<()> {
    if let Some(previous) = portable_paths.insert(collision_key.to_vec(), relative.to_path_buf()) {
        let mut paths = [previous, relative.to_path_buf()];
        paths.sort();
        anyhow::bail!(
            "portable path collision: {} and {}",
            paths[0].display(),
            paths[1].display()
        );
    }
    Ok(())
}

fn validate_portable_component(component: &str, relative: &Path) -> anyhow::Result<()> {
    if component.contains('\\') {
        anyhow::bail!(
            "package path components cannot contain a backslash: {}",
            relative.display()
        );
    }
    if component.chars().any(|character| {
        character.is_control() || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    }) {
        anyhow::bail!(
            "package path component is not portable across platforms: {}",
            relative.display()
        );
    }
    if component.ends_with([' ', '.']) || is_windows_reserved_name(component) {
        anyhow::bail!(
            "package path component is not portable across platforms: {}",
            relative.display()
        );
    }
    Ok(())
}

fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}
