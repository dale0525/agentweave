use crate::skill_package::{SkillPackageDescriptor, SkillPackageId};
use crate::skill_source::{canonical_relative_path, hash_package_tree};
use crate::skill_state::{
    NewSkillRevision, SkillRevisionMetadata, SkillRevisionPromotion, SkillRevisionRecord,
    SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store_fs::{
    PackageLimits, StoreFaultPoint, StoreFaults, copy_package_tree, ensure_safe_write_parent,
    make_tree_readonly, make_tree_writable, measure_package_tree,
};
use anyhow::Context;
use serde_json::json;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_SKILL_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_SKILL_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
pub(crate) use crate::skill_store_fs::{
    StoreFaultPoint as SkillStoreFaultPoint, StoreFaults as SkillStoreTestFaults,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillStoreLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
}

impl Default for SkillStoreLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_SKILL_FILE_BYTES,
            max_package_bytes: DEFAULT_MAX_SKILL_PACKAGE_BYTES,
        }
    }
}

impl SkillStoreLimits {
    fn package_limits(self) -> PackageLimits {
        PackageLimits {
            max_file_bytes: self.max_file_bytes,
            max_package_bytes: self.max_package_bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillStorePaths {
    pub managed: PathBuf,
    pub staging: PathBuf,
    pub quarantine: PathBuf,
}

impl SkillStorePaths {
    pub async fn prepare(app_data_root: &Path, cache_root: &Path) -> anyhow::Result<Self> {
        let paths = Self {
            managed: app_data_root.join("managed-skills"),
            staging: cache_root.join("skill-staging"),
            quarantine: app_data_root.join("skill-quarantine"),
        };
        for path in [&paths.managed, &paths.staging, &paths.quarantine] {
            tokio::fs::create_dir_all(path)
                .await
                .with_context(|| format!("failed to prepare skill store {}", path.display()))?;
            let metadata = tokio::fs::symlink_metadata(path)
                .await
                .with_context(|| format!("failed to inspect skill store {}", path.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                anyhow::bail!(
                    "skill store root must be a real directory: {}",
                    path.display()
                );
            }
        }
        Ok(paths)
    }
}

#[derive(Clone, Debug)]
pub struct StoredSkillRevision {
    pub revision_id: String,
    pub package_id: SkillPackageId,
    pub path: PathBuf,
    pub content_hash: String,
}

#[derive(Clone)]
pub struct SkillRevisionStore {
    paths: SkillStorePaths,
    state: SkillStateStore,
    limits: SkillStoreLimits,
    faults: StoreFaults,
}

impl SkillRevisionStore {
    pub fn new(paths: SkillStorePaths, state: SkillStateStore) -> Self {
        Self::with_limits(paths, state, SkillStoreLimits::default())
    }

    pub fn with_limits(
        paths: SkillStorePaths,
        state: SkillStateStore,
        limits: SkillStoreLimits,
    ) -> Self {
        Self {
            paths,
            state,
            limits,
            faults: StoreFaults::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_faults(
        paths: SkillStorePaths,
        state: SkillStateStore,
        limits: SkillStoreLimits,
        faults: SkillStoreTestFaults,
    ) -> Self {
        Self {
            paths,
            state,
            limits,
            faults,
        }
    }

    pub fn paths(&self) -> &SkillStorePaths {
        &self.paths
    }

    pub async fn create_staging_revision(
        &self,
        source: &Path,
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        let revision_id = SkillStateStore::allocate_revision_id();
        let destination = self.paths.staging.join(&revision_id);
        let result = async {
            copy_package_tree(
                source,
                &destination,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            let loaded = SkillPackageDescriptor::load(&destination).await?;
            loaded.descriptor.validate()?;
            measure_package_tree(&destination, self.limits.package_limits(), None).await?;
            let content_hash = hash_package_tree(&destination).await?;
            let record = self
                .state
                .create_staging_revision_record(
                    &revision_id,
                    NewSkillRevision {
                        package_id: loaded.descriptor.id.clone(),
                        version: loaded.descriptor.version.to_string(),
                        content_hash: content_hash.clone(),
                        storage_path: storage_path(&destination)?,
                        descriptor_json: serde_json::to_value(&loaded.descriptor)?,
                        validation_json: json!({"status": "pending"}),
                        created_by: actor_id.to_string(),
                    },
                )
                .await?;
            Ok(stored_revision(record, destination.clone()))
        }
        .await;
        match result {
            Ok(revision) => Ok(revision),
            Err(error) => match remove_tree_if_exists(&destination).await {
                Ok(()) => Err(error),
                Err(compensation) => Err(with_compensation(error, compensation)),
            },
        }
    }

    pub async fn write_staging_file(
        &self,
        revision_id: &str,
        relative_path: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        canonical_relative_path(relative_path)?;
        let record = self.staging_record(revision_id).await?;
        let root = PathBuf::from(&record.storage_path);
        let expected = self.paths.staging.join(revision_id);
        ensure_exact_path(&root, &expected, "staging")?;
        let byte_count = u64::try_from(bytes.len())?;
        if byte_count > self.limits.max_file_bytes {
            anyhow::bail!(
                "staging file exceeds {} byte limit",
                self.limits.max_file_bytes
            );
        }
        let existing_bytes =
            measure_package_tree(&root, self.limits.package_limits(), Some(relative_path)).await?;
        let final_bytes = existing_bytes
            .checked_add(byte_count)
            .context("skill package byte count overflow")?;
        if final_bytes > self.limits.max_package_bytes {
            anyhow::bail!(
                "skill package exceeds {} byte limit",
                self.limits.max_package_bytes
            );
        }
        ensure_safe_write_parent(&root, relative_path).await?;
        let destination = root.join(relative_path);
        let previous = match tokio::fs::read(&destination).await {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        tokio::fs::write(&destination, bytes).await?;
        let update = self.refresh_staging_metadata(&record, &root).await;
        if let Err(error) = update {
            let compensation = restore_file(&destination, previous.as_deref()).await;
            return match compensation {
                Ok(()) => Err(error),
                Err(compensation) => Err(with_compensation(error, compensation)),
            };
        }
        Ok(())
    }

    pub async fn promote_revision(&self, revision_id: &str) -> anyhow::Result<StoredSkillRevision> {
        let record = self.staging_record(revision_id).await?;
        let staged = PathBuf::from(&record.storage_path);
        ensure_exact_path(&staged, &self.paths.staging.join(revision_id), "staging")?;
        let final_metadata = self.final_metadata(&record, &staged).await?;
        let package_root = self
            .paths
            .managed
            .join(record.package_id.as_str())
            .join("revisions");
        let incoming_root = self.paths.managed.join(".incoming");
        let promoting_root = self.paths.staging.join(".promoting");
        tokio::fs::create_dir_all(&package_root).await?;
        tokio::fs::create_dir_all(&incoming_root).await?;
        tokio::fs::create_dir_all(&promoting_root).await?;
        let incoming = incoming_root.join(revision_id);
        let destination = package_root.join(revision_id);
        let promoting = promoting_root.join(revision_id);
        let destination_storage = storage_path(&destination)?;
        reject_path_exists(&destination).await?;
        reject_path_exists(&promoting).await?;

        let copied = copy_package_tree(
            &staged,
            &incoming,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::IncomingCopyFile,
        )
        .await;
        if let Err(error) = copied {
            return cleanup_incoming_error(error, &incoming).await;
        }
        let incoming_hash = match hash_package_tree(&incoming).await {
            Ok(hash) => hash,
            Err(error) => return cleanup_incoming_error(error, &incoming).await,
        };
        if incoming_hash != final_metadata.content_hash {
            return cleanup_incoming_error(
                anyhow::anyhow!("staging copy hash mismatch"),
                &incoming,
            )
            .await;
        }

        let staging_rename = async {
            self.faults.check(StoreFaultPoint::PromoteStagingRename)?;
            tokio::fs::rename(&staged, &promoting).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = staging_rename {
            return cleanup_incoming_error(error, &incoming).await;
        }
        let rename_result = async {
            self.faults.check(StoreFaultPoint::PromoteIncomingRename)?;
            tokio::fs::rename(&incoming, &destination).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = rename_result {
            let restore = tokio::fs::rename(&promoting, &staged).await;
            let cleanup = remove_tree_if_exists(&incoming).await;
            return match (restore, cleanup) {
                (Ok(()), Ok(())) => Err(error),
                (restore, cleanup) => Err(with_compensation(
                    error,
                    anyhow::anyhow!(
                        "staging restore: {:?}; incoming cleanup: {:?}",
                        restore.err(),
                        cleanup.err()
                    ),
                )),
            };
        }

        let promotion = SkillRevisionPromotion {
            version: final_metadata.version.clone(),
            content_hash: final_metadata.content_hash.clone(),
            storage_path: destination_storage,
            descriptor_json: final_metadata.descriptor_json.clone(),
            validation_json: final_metadata.validation_json.clone(),
        };
        let database_result = async {
            self.faults.check(StoreFaultPoint::PromoteDatabase)?;
            self.state
                .promote_revision_record_with_metadata(revision_id, promotion)
                .await
        }
        .await;
        let promoted = match database_result {
            Ok(record) => record,
            Err(error) => {
                return self
                    .compensate_failed_promotion(error, &record, &staged, &promoting, &destination)
                    .await;
            }
        };
        remove_tree_if_exists(&promoting).await?;
        let _ = make_tree_readonly(&destination, self.limits.package_limits()).await;
        Ok(stored_revision(promoted, destination))
    }

    pub async fn quarantine_revision(
        &self,
        revision_id: &str,
        reason: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("skill revision not found: {revision_id}"))?;
        let source = self.expected_revision_path(&record)?;
        measure_package_tree(&source, self.limits.package_limits(), None).await?;
        let quarantine_incoming_root = self.paths.quarantine.join(".incoming");
        tokio::fs::create_dir_all(&quarantine_incoming_root).await?;
        let incoming = quarantine_incoming_root.join(revision_id);
        let destination = self.paths.quarantine.join(revision_id);
        let backup = source
            .parent()
            .context("revision source has no parent")?
            .join(format!(".quarantining-{revision_id}"));
        reject_path_exists(&destination).await?;
        reject_path_exists(&backup).await?;
        let copied = copy_package_tree(
            &source,
            &incoming,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::QuarantineCopyFile,
        )
        .await;
        if let Err(error) = copied {
            return cleanup_incoming_error(error, &incoming).await;
        }
        let incoming_rename = async {
            self.faults
                .check(StoreFaultPoint::QuarantineIncomingRename)?;
            tokio::fs::rename(&incoming, &destination).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = incoming_rename {
            return cleanup_incoming_error(error, &incoming).await;
        }
        if record.status == SkillRevisionStatus::Managed {
            make_tree_writable(&source, self.limits.package_limits()).await?;
        }
        let source_rename = async {
            self.faults.check(StoreFaultPoint::QuarantineSourceRename)?;
            tokio::fs::rename(&source, &backup).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = source_rename {
            if record.status == SkillRevisionStatus::Managed {
                let _ = make_tree_readonly(&source, self.limits.package_limits()).await;
            }
            return match remove_tree_if_exists(&destination).await {
                Ok(()) => Err(error),
                Err(compensation) => Err(with_compensation(error, compensation)),
            };
        }

        let database_result = async {
            self.faults.check(StoreFaultPoint::QuarantineDatabase)?;
            self.state
                .quarantine_revision_record(revision_id, &storage_path(&destination)?, reason)
                .await
        }
        .await;
        let quarantined = match database_result {
            Ok(record) => record,
            Err(error) => {
                return self
                    .compensate_failed_quarantine(
                        error,
                        &record,
                        &source,
                        &backup,
                        &destination,
                        reason,
                    )
                    .await;
            }
        };
        remove_tree_if_exists(&backup).await?;
        Ok(stored_revision(quarantined, destination))
    }

    async fn staging_record(&self, revision_id: &str) -> anyhow::Result<SkillRevisionRecord> {
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("skill revision not found: {revision_id}"))?;
        if record.status != SkillRevisionStatus::Staging {
            anyhow::bail!("revision is not an editable staging revision: {revision_id}");
        }
        Ok(record)
    }

    async fn refresh_staging_metadata(
        &self,
        record: &SkillRevisionRecord,
        root: &Path,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let metadata = self.final_metadata(record, root).await?;
        self.state
            .refresh_staging_revision_metadata(&record.revision_id, metadata)
            .await
    }

    async fn final_metadata(
        &self,
        record: &SkillRevisionRecord,
        root: &Path,
    ) -> anyhow::Result<SkillRevisionMetadata> {
        measure_package_tree(root, self.limits.package_limits(), None).await?;
        let loaded = SkillPackageDescriptor::load(root).await?;
        loaded.descriptor.validate()?;
        if loaded.descriptor.id != record.package_id {
            anyhow::bail!(
                "revision descriptor package {} does not match record package {}",
                loaded.descriptor.id.as_str(),
                record.package_id.as_str()
            );
        }
        Ok(SkillRevisionMetadata {
            version: loaded.descriptor.version.to_string(),
            content_hash: hash_package_tree(root).await?,
            descriptor_json: serde_json::to_value(&loaded.descriptor)?,
            validation_json: json!({"status": "valid"}),
        })
    }

    fn expected_revision_path(&self, record: &SkillRevisionRecord) -> anyhow::Result<PathBuf> {
        let expected = match record.status {
            SkillRevisionStatus::Staging => self.paths.staging.join(&record.revision_id),
            SkillRevisionStatus::Managed => self
                .paths
                .managed
                .join(record.package_id.as_str())
                .join("revisions")
                .join(&record.revision_id),
            SkillRevisionStatus::Quarantined => {
                anyhow::bail!("revision is already quarantined: {}", record.revision_id)
            }
        };
        ensure_exact_path(Path::new(&record.storage_path), &expected, "revision")?;
        Ok(expected)
    }

    async fn compensate_failed_promotion(
        &self,
        error: anyhow::Error,
        record: &SkillRevisionRecord,
        staged: &Path,
        promoting: &Path,
        destination: &Path,
    ) -> anyhow::Result<StoredSkillRevision> {
        let restore = async {
            self.faults.check(StoreFaultPoint::PromoteRestoreRename)?;
            tokio::fs::rename(promoting, staged).await?;
            remove_tree_if_exists(destination).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(compensation) = restore {
            let quarantine = self.paths.quarantine.join(&record.revision_id);
            reject_path_exists(&quarantine).await?;
            tokio::fs::rename(destination, &quarantine).await?;
            let compensation_message = format!("{compensation:#}");
            let quarantine_result = self
                .state
                .quarantine_revision_record(
                    &record.revision_id,
                    &storage_path(&quarantine)?,
                    "promotion compensation failed",
                )
                .await;
            return match quarantine_result {
                Ok(quarantined) => match remove_tree_if_exists(promoting).await {
                    Ok(()) => Err(error.context(format!(
                        "compensation failed: {compensation_message}; revision quarantined at {} with status {}",
                        quarantine.display(),
                        quarantined.status.as_str()
                    ))),
                    Err(cleanup_error) => Err(error.context(format!(
                        "compensation failed: {compensation_message}; revision quarantined at {} but backup cleanup failed: {cleanup_error:#}",
                        quarantine.display()
                    ))),
                },
                Err(quarantine_error) => {
                    let quarantine_error_message = format!("{quarantine_error:#}");
                    let fallback = async {
                        tokio::fs::rename(promoting, staged).await?;
                        remove_tree_if_exists(&quarantine).await?;
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    match fallback {
                        Ok(()) => Err(error.context(format!(
                            "compensation failed: {compensation_message}; quarantine persistence failed: {quarantine_error_message}; fallback restored staging"
                        ))),
                        Err(fallback_error) => Err(error.context(format!(
                            "compensation failed: {compensation_message}; quarantine persistence failed: {quarantine_error_message}; fallback restore failed: {fallback_error:#}"
                        ))),
                    }
                }
            };
        }
        Err(error)
    }

    async fn compensate_failed_quarantine(
        &self,
        error: anyhow::Error,
        record: &SkillRevisionRecord,
        source: &Path,
        backup: &Path,
        destination: &Path,
        reason: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        let restore = async {
            self.faults
                .check(StoreFaultPoint::QuarantineRestoreRename)?;
            tokio::fs::rename(backup, source).await?;
            remove_tree_if_exists(destination).await?;
            if record.status == SkillRevisionStatus::Managed {
                let _ = make_tree_readonly(source, self.limits.package_limits()).await;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(compensation) = restore {
            let compensation_message = format!("{compensation:#}");
            let persisted = self
                .state
                .quarantine_revision_record(
                    &record.revision_id,
                    &storage_path(destination)?,
                    reason,
                )
                .await;
            return match persisted {
                Ok(persisted) => match remove_tree_if_exists(backup).await {
                    Ok(()) => Err(error.context(format!(
                        "compensation failed: {compensation_message}; content retained in quarantine with status {}",
                        persisted.status.as_str()
                    ))),
                    Err(cleanup_error) => Err(error.context(format!(
                        "compensation failed: {compensation_message}; content retained in quarantine but backup cleanup failed: {cleanup_error:#}"
                    ))),
                },
                Err(quarantine_error) => {
                    let quarantine_error_message = format!("{quarantine_error:#}");
                    let fallback = async {
                        tokio::fs::rename(backup, source).await?;
                        remove_tree_if_exists(destination).await?;
                        if record.status == SkillRevisionStatus::Managed {
                            let _ = make_tree_readonly(source, self.limits.package_limits()).await;
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    match fallback {
                        Ok(()) => Err(error.context(format!(
                            "compensation failed: {compensation_message}; quarantine persistence failed: {quarantine_error_message}; fallback restored original path"
                        ))),
                        Err(fallback_error) => Err(error.context(format!(
                            "compensation failed: {compensation_message}; quarantine persistence failed: {quarantine_error_message}; fallback restore failed: {fallback_error:#}"
                        ))),
                    }
                }
            };
        }
        Err(error)
    }
}

fn stored_revision(record: SkillRevisionRecord, path: PathBuf) -> StoredSkillRevision {
    StoredSkillRevision {
        revision_id: record.revision_id,
        package_id: record.package_id,
        path,
        content_hash: record.content_hash,
    }
}

fn ensure_exact_path(actual: &Path, expected: &Path, label: &str) -> anyhow::Result<()> {
    if actual != expected {
        anyhow::bail!(
            "{label} storage path mismatch: expected {}, found {}",
            expected.display(),
            actual.display()
        );
    }
    Ok(())
}

fn storage_path(path: &Path) -> anyhow::Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .with_context(|| format!("skill storage path must be UTF-8: {}", path.display()))
}

async fn reject_path_exists(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => anyhow::bail!("skill store destination already exists: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn remove_tree_if_exists(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn restore_file(path: &Path, previous: Option<&[u8]>) -> anyhow::Result<()> {
    match previous {
        Some(bytes) => tokio::fs::write(path, bytes).await?,
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

async fn cleanup_incoming_error<T>(error: anyhow::Error, incoming: &Path) -> anyhow::Result<T> {
    match remove_tree_if_exists(incoming).await {
        Ok(()) => Err(error),
        Err(compensation) => Err(with_compensation(error, compensation)),
    }
}

fn with_compensation(primary: anyhow::Error, compensation: anyhow::Error) -> anyhow::Error {
    primary.context(format!("compensation failed: {compensation:#}"))
}
