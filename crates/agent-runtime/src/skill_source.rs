use crate::skill_package::SkillPackageDescriptor;
use crate::skill_state::{
    SkillLayerRecord, SkillRevisionRecord, SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store::{SkillRevisionStore, SkillStoreLimits, SkillStorePaths};
use crate::skill_store_secure_fs::{
    secure_package_hash, secure_package_snapshot, unbounded_package_limits,
};
use crate::skill_store_secure_roots::package_snapshot as prepared_package_snapshot;
use anyhow::Context;
use async_trait::async_trait;
use icu_casemap::CaseMapper;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use unicode_normalization::UnicodeNormalization;

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
    /// Discovery-bound bytes used to build managed snapshots without reopening mutable paths.
    pub verified_content: Option<VerifiedPackageContent>,
}

/// Small, security-sensitive package inputs captured by managed discovery.
#[derive(Clone, Debug)]
pub struct VerifiedPackageContent {
    /// Verified `skill.json` bytes, when runtime content is declared.
    pub runtime_manifest: Option<Arc<[u8]>>,
    /// Verified `SKILL.md` bytes, when instruction content is declared.
    pub instructions_file: Option<Arc<[u8]>>,
    /// Hash of the complete package tree observed with these bytes.
    pub expected_content_hash: String,
    /// Bounds that must also be applied when rechecking before execution.
    pub limits: SkillStoreLimits,
    pub(crate) execution_binding: Option<ManagedExecutionBinding>,
}

#[derive(Clone)]
pub(crate) struct ManagedExecutionBinding {
    pub(crate) store: SkillRevisionStore,
    pub(crate) package_id: crate::skill_package::SkillPackageId,
    pub(crate) revision_id: String,
    pub(crate) storage_path: PathBuf,
}

impl std::fmt::Debug for ManagedExecutionBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedExecutionBinding")
            .field("package_id", &self.package_id)
            .field("revision_id", &self.revision_id)
            .field("storage_path", &self.storage_path)
            .finish_non_exhaustive()
    }
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
            let snapshot = secure_package_snapshot(&root, unbounded_package_limits()).await?;
            let loaded = snapshot.descriptor;
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
                content_hash: snapshot.content_hash,
                root: loaded.root,
                descriptor: loaded.descriptor,
                warnings: loaded.warnings,
                verified_content: None,
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
    pub diagnostic_error: Option<String>,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
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

    pub fn from_store(store: SkillRevisionStore) -> Self {
        let paths = store.paths().clone();
        let state = store.state_store();
        Self {
            paths,
            state,
            store,
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
    ) -> anyhow::Result<DiscoveredSkillPackage> {
        self.paths.verify_identity()?;
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
        self.store.check_managed_discovery_io()?;
        let relative = PathBuf::from(revision.package_id.as_str())
            .join("revisions")
            .join(&revision.revision_id);
        let snapshot = prepared_package_snapshot(
            self.paths.managed_identity(),
            &relative,
            self.store.package_limits(),
        )
        .await
        .with_context(|| {
            format!(
                "failed to inspect managed revision {}",
                stored_path.display()
            )
        })?;
        let loaded = snapshot.descriptor;
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
        let content_hash = snapshot.content_hash;
        if content_hash != revision.content_hash {
            anyhow::bail!(
                "managed content hash mismatch for revision {}",
                revision.revision_id
            );
        }
        Ok(DiscoveredSkillPackage {
            layer: SkillLayer::Managed,
            root: stored_path.clone(),
            descriptor: loaded.descriptor,
            content_hash: content_hash.clone(),
            warnings: loaded.warnings,
            verified_content: Some(VerifiedPackageContent {
                runtime_manifest: snapshot.runtime_manifest.map(Arc::from),
                instructions_file: snapshot.instructions_file.map(Arc::from),
                expected_content_hash: content_hash,
                limits: self.store.limits,
                execution_binding: Some(ManagedExecutionBinding {
                    store: self.store.clone(),
                    package_id: revision.package_id,
                    revision_id: revision.revision_id,
                    storage_path: stored_path.clone(),
                }),
            }),
        })
    }
}

#[async_trait]
impl SkillSource for ManagedSkillSource {
    fn layer(&self) -> SkillLayer {
        SkillLayer::Managed
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        self.issues
            .write()
            .expect("managed skill issue lock poisoned")
            .clear();
        self.paths.verify_identity()?;
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
        let installations = self.state.list_active_installations().await?;
        let mut discovered = Vec::new();
        let mut issues = Vec::new();
        for installation in installations {
            if installation.source_layer != SkillLayerRecord::Managed {
                continue;
            }
            let revision_id = installation
                .active_revision_id
                .as_deref()
                .expect("active installation invariant validated by state rows");
            match self
                .validate_revision(
                    &installation.package_id,
                    installation.source_layer,
                    revision_id,
                )
                .await
            {
                Ok(package) => discovered.push(package),
                Err(error) => {
                    let transient = is_transient_discovery_error(&error);
                    let reason = format!("{error:#}");
                    let quarantine_error = if transient {
                        None
                    } else {
                        self.store
                            .quarantine_revision(revision_id, &reason)
                            .await
                            .err()
                            .map(|error| format!("{error:#}"))
                    };
                    let diagnostic_error = if let Some(quarantine_error) = &quarantine_error {
                        self.state
                            .record_revision_diagnostic(
                                &installation.package_id,
                                revision_id,
                                "managed_discovery_quarantine_failed",
                                json!({
                                    "reason": reason,
                                    "quarantine_error": quarantine_error,
                                }),
                            )
                            .await
                            .err()
                            .map(|error| format!("{error:#}"))
                    } else {
                        None
                    };
                    issues.push(ManagedSkillIssue {
                        package_id: installation.package_id.as_str().to_string(),
                        revision_id: revision_id.to_string(),
                        reason,
                        quarantine_error,
                        diagnostic_error,
                        recorded_at: chrono::Utc::now(),
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

fn is_transient_discovery_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let io_transient = cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::OutOfMemory
            ) || error.raw_os_error().is_some_and(is_transient_raw_os_error)
        });
        #[cfg(unix)]
        let errno_transient = cause
            .downcast_ref::<rustix::io::Errno>()
            .is_some_and(|error| {
                matches!(
                    *error,
                    rustix::io::Errno::MFILE
                        | rustix::io::Errno::NFILE
                        | rustix::io::Errno::INTR
                        | rustix::io::Errno::AGAIN
                        | rustix::io::Errno::ACCESS
                        | rustix::io::Errno::TIMEDOUT
                )
            });
        #[cfg(not(unix))]
        let errno_transient = false;
        io_transient || errno_transient
    })
}

#[cfg(any(test, windows))]
fn is_windows_transient_raw_os_error(code: i32) -> bool {
    matches!(code, 4 | 32 | 33)
}

fn is_transient_raw_os_error(code: i32) -> bool {
    #[cfg(windows)]
    {
        is_windows_transient_raw_os_error(code)
    }
    #[cfg(unix)]
    {
        matches!(code, 23 | 24)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = code;
        false
    }
}

#[cfg(test)]
mod transient_error_tests {
    use super::is_windows_transient_raw_os_error;

    #[test]
    fn windows_resource_and_sharing_errors_are_transient() {
        for code in [4, 32, 33] {
            assert!(
                is_windows_transient_raw_os_error(code),
                "Win32 error {code} must skip discovery without quarantine"
            );
        }
    }

    #[test]
    fn windows_integrity_errors_are_not_transient() {
        for code in [2, 3, 4390] {
            assert!(
                !is_windows_transient_raw_os_error(code),
                "Win32 error {code} must remain an integrity failure"
            );
        }
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
    secure_package_hash(root, unbounded_package_limits()).await
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
