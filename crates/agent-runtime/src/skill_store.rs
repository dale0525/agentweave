use crate::skill_package::SkillPackageId;
use crate::skill_source::canonical_relative_path;
use crate::skill_state::{
    NewSkillRevision, SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion,
    SkillRevisionRecord, SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store_atomic_write::atomic_replace_file;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs::{
    PackageLimits, copy_package_tree_into_prepared, copy_prepared_package_tree_into_prepared,
    ensure_directory_contained, ensure_safe_write_parent, make_tree_readonly, make_tree_writable,
    measure_package_tree, read_optional_regular_file, remove_created_directories,
    remove_regular_file_nofollow,
};
use crate::skill_store_fs_types::AtomicReplaceCommitState;
use crate::skill_store_locks::{SkillStoreIdentity, acquire_revision_lock};
use crate::skill_store_operations::{
    TransitionPhase, TransitionState, cleanup_created_directories_error, combine_operation_errors,
    ensure_exact_path, storage_path, stored_revision, with_compensation,
};
use crate::skill_store_path_prepare::prepare_canonical_directory;
use crate::skill_store_secure_fs::ensure_store_directory;
use crate::skill_store_secure_roots::{
    ensure_directory as ensure_prepared_directory, open_prepared_directory,
    opened_package_snapshot, remove_opened_tree as remove_opened_prepared_tree,
    reserve_opened_directory as reserve_opened_prepared_directory,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub const DEFAULT_MAX_SKILL_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_SKILL_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_SKILL_ENTRIES: u64 = 4096;
pub const DEFAULT_MAX_SKILL_FILES: u64 = 2048;
pub const DEFAULT_MAX_SKILL_DIRECTORIES: u64 = 2048;
pub const DEFAULT_MAX_SKILL_DEPTH: u64 = 32;
pub const DEFAULT_MAX_SKILL_RELATIVE_PATH_BYTES: u64 = 4096;

#[cfg(test)]
pub(crate) use crate::skill_store_faults::{
    StoreFaultPoint as SkillStoreFaultPoint, StoreFaults as SkillStoreTestFaults,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillStoreLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
    pub max_entries: u64,
    pub max_files: u64,
    pub max_directories: u64,
    pub max_depth: u64,
    pub max_relative_path_bytes: u64,
}

impl Default for SkillStoreLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_SKILL_FILE_BYTES,
            max_package_bytes: DEFAULT_MAX_SKILL_PACKAGE_BYTES,
            max_entries: DEFAULT_MAX_SKILL_ENTRIES,
            max_files: DEFAULT_MAX_SKILL_FILES,
            max_directories: DEFAULT_MAX_SKILL_DIRECTORIES,
            max_depth: DEFAULT_MAX_SKILL_DEPTH,
            max_relative_path_bytes: DEFAULT_MAX_SKILL_RELATIVE_PATH_BYTES,
        }
    }
}

impl SkillStoreLimits {
    pub(crate) fn package_limits(self) -> PackageLimits {
        PackageLimits {
            max_file_bytes: self.max_file_bytes,
            max_package_bytes: self.max_package_bytes,
            max_entries: self.max_entries,
            max_files: self.max_files,
            max_directories: self.max_directories,
            max_depth: self.max_depth,
            max_relative_path_bytes: self.max_relative_path_bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillStorePaths {
    pub managed: PathBuf,
    pub staging: PathBuf,
    pub quarantine: PathBuf,
    pub(crate) identity: SkillStoreIdentity,
}

impl SkillStorePaths {
    pub async fn prepare(app_data_root: &Path, cache_root: &Path) -> anyhow::Result<Self> {
        let app_data_root = prepare_canonical_directory(app_data_root).await?;
        let cache_root = prepare_canonical_directory(cache_root).await?;
        ensure_store_directory(&app_data_root, Path::new("managed-skills"))
            .await
            .with_context(|| {
                format!(
                    "skill store root must be a real directory: {}",
                    app_data_root.join("managed-skills").display()
                )
            })?;
        ensure_store_directory(&app_data_root, Path::new("skill-quarantine")).await?;
        ensure_store_directory(&cache_root, Path::new("skill-staging")).await?;
        let managed = app_data_root.join("managed-skills");
        let staging = cache_root.join("skill-staging");
        let quarantine = app_data_root.join("skill-quarantine");
        ensure_store_directory(&managed, Path::new(".locks")).await?;
        for path in [&managed, &staging, &quarantine] {
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
        let identity = SkillStoreIdentity::capture(
            tokio::fs::canonicalize(&managed).await?,
            tokio::fs::canonicalize(&staging).await?,
            tokio::fs::canonicalize(&quarantine).await?,
        )?;
        Ok(Self {
            managed,
            staging,
            quarantine,
            identity,
        })
    }

    pub(crate) fn verify_identity(&self) -> anyhow::Result<()> {
        self.identity.verify()
    }

    pub(crate) fn managed_identity(&self) -> &crate::skill_store_locks::StoreRootIdentity {
        self.identity.managed()
    }

    pub(crate) fn staging_identity(&self) -> &crate::skill_store_locks::StoreRootIdentity {
        self.identity.staging()
    }

    pub(crate) fn quarantine_identity(&self) -> &crate::skill_store_locks::StoreRootIdentity {
        self.identity.quarantine()
    }
}

#[derive(Clone, Debug)]
pub struct StoredSkillRevision {
    pub revision_id: String,
    pub package_id: SkillPackageId,
    pub path: PathBuf,
    pub content_hash: String,
    pub maintenance_issues: Vec<SkillStoreMaintenanceIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillStoreMaintenanceIssue {
    pub revision_id: String,
    pub operation: String,
    pub path: PathBuf,
    pub message: String,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SkillRevisionStore {
    pub(crate) paths: SkillStorePaths,
    pub(crate) state: SkillStateStore,
    pub(crate) limits: SkillStoreLimits,
    pub(crate) faults: StoreFaults,
    pub(crate) maintenance_issues: Arc<RwLock<Vec<SkillStoreMaintenanceIssue>>>,
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
            maintenance_issues: Arc::new(RwLock::new(Vec::new())),
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
            maintenance_issues: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn maintenance_issues(&self) -> Vec<SkillStoreMaintenanceIssue> {
        self.maintenance_issues
            .read()
            .expect("skill store maintenance issue lock poisoned")
            .clone()
    }

    pub fn paths(&self) -> &SkillStorePaths {
        &self.paths
    }

    pub(crate) fn state_store(&self) -> SkillStateStore {
        self.state.clone()
    }

    pub(crate) fn package_limits(&self) -> PackageLimits {
        self.limits.package_limits()
    }

    pub(crate) fn check_managed_discovery_io(&self) -> anyhow::Result<()> {
        self.faults
            .check(StoreFaultPoint::ManagedDiscoveryTransientIo)
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    format!("transient managed discovery I/O: {error:#}"),
                )
                .into()
            })
    }

    pub async fn create_staging_revision(
        &self,
        source: &Path,
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        self.paths.verify_identity()?;
        ensure_directory_contained(&self.paths.staging, &self.paths.staging, "staging").await?;
        let revision_id = self
            .faults
            .take_revision_id()
            .unwrap_or_else(SkillStateStore::allocate_revision_id);
        let destination = self.paths.staging.join(&revision_id);
        let mut reserved = None;
        let result = async {
            let reserved_directory = reserve_opened_prepared_directory(
                self.paths.staging_identity(),
                Path::new(&revision_id),
            )
            .await?;
            reserved = Some(reserved_directory.clone());
            self.paths.verify_identity()?;
            copy_package_tree_into_prepared(
                source,
                &reserved_directory,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            make_tree_writable(&reserved_directory, self.limits.package_limits()).await?;
            let snapshot =
                opened_package_snapshot(&reserved_directory, self.limits.package_limits()).await?;
            let loaded = snapshot.descriptor;
            loaded.descriptor.validate()?;
            let content_hash = snapshot.content_hash;
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
            Ok(stored_revision(record, destination.clone(), Vec::new()))
        }
        .await;
        match result {
            Ok(revision) => Ok(revision),
            Err(error) if reserved.is_none() => Err(error),
            Err(error) => {
                match remove_opened_prepared_tree(reserved.as_ref().expect("reservation recorded"))
                    .await
                {
                    Ok(()) => Err(error),
                    Err(compensation) => Err(with_compensation(error, compensation)),
                }
            }
        }
    }

    pub async fn write_staging_file(
        &self,
        revision_id: &str,
        relative_path: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        canonical_relative_path(relative_path)?;
        let observed = self.staging_record(revision_id).await?;
        let _revision_guard =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::WriteAfterLock)
            .await;
        self.paths.verify_identity()?;
        let root = PathBuf::from(&record.storage_path);
        let expected = self.paths.staging.join(revision_id);
        ensure_exact_path(&root, &expected, "staging")?;
        ensure_directory_contained(&self.paths.staging, &root, "staging").await?;
        let prepared_root =
            open_prepared_directory(self.paths.staging_identity(), Path::new(revision_id)).await?;
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
        let created_directories = ensure_safe_write_parent(&root, relative_path).await?;
        let previous = match read_optional_regular_file(
            &root,
            relative_path,
            self.limits.max_file_bytes,
        )
        .await
        {
            Ok(previous) => previous,
            Err(error) => {
                return cleanup_created_directories_error(error, &created_directories).await;
            }
        };
        let mode = previous.as_ref().map_or(0o644, |file| file.mode);
        if let Err(failure) =
            atomic_replace_file(&prepared_root, relative_path, bytes, mode, &self.faults).await
        {
            let state = failure.state;
            if let Some(temp_path) = &failure.temp_path {
                self.record_maintenance_issue(
                    revision_id,
                    "staging_write_temp_cleanup",
                    temp_path,
                    &anyhow::anyhow!("temporary file cleanup failed"),
                );
            }
            let error = failure.into_error();
            if state == AtomicReplaceCommitState::NotCommitted {
                let directory_cleanup = remove_created_directories(&created_directories).await;
                return Err(combine_operation_errors(
                    error,
                    [("parent cleanup", directory_cleanup)],
                ));
            }
            let restore: anyhow::Result<()> = match &previous {
                Some(previous) => atomic_replace_file(
                    &prepared_root,
                    relative_path,
                    &previous.bytes,
                    previous.mode,
                    &self.faults,
                )
                .await
                .map_err(|failure| failure.into_error()),
                None => remove_regular_file_nofollow(&root, relative_path).await,
            };
            if let Err(restore_error) = restore {
                let directory_cleanup = remove_created_directories(&created_directories).await;
                let primary = combine_operation_errors(
                    error,
                    [
                        ("file restore", Err(restore_error)),
                        ("parent cleanup", directory_cleanup),
                    ],
                );
                return Err(self
                    .isolate_failed_staging_write(
                        &record,
                        &root,
                        &prepared_root,
                        relative_path,
                        previous.as_ref(),
                        primary,
                    )
                    .await);
            }
            let directory_cleanup = remove_created_directories(&created_directories).await;
            return Err(combine_operation_errors(
                error,
                [("parent cleanup", directory_cleanup)],
            ));
        }
        self.faults
            .checkpoint(StoreFaultPoint::WriteBeforeMetadataCommit)
            .await;
        let update = self.refresh_staging_metadata(&record, &prepared_root).await;
        if let Err(error) = update {
            let restore = async {
                self.faults.check(StoreFaultPoint::WriteRestore)?;
                match &previous {
                    Some(previous) => atomic_replace_file(
                        &prepared_root,
                        relative_path,
                        &previous.bytes,
                        previous.mode,
                        &self.faults,
                    )
                    .await
                    .map_err(|failure| failure.into_error()),
                    None => remove_regular_file_nofollow(&root, relative_path).await,
                }
            }
            .await;
            if let Err(restore_error) = restore {
                let directory_cleanup = remove_created_directories(&created_directories).await;
                let primary = combine_operation_errors(
                    error,
                    [
                        ("file restore", Err(restore_error)),
                        ("parent cleanup", directory_cleanup),
                    ],
                );
                return Err(self
                    .isolate_failed_staging_write(
                        &record,
                        &root,
                        &prepared_root,
                        relative_path,
                        previous.as_ref(),
                        primary,
                    )
                    .await);
            }
            let directory_cleanup = remove_created_directories(&created_directories).await;
            return Err(combine_operation_errors(
                error,
                [("parent cleanup", directory_cleanup)],
            ));
        }
        Ok(())
    }

    pub async fn promote_revision(&self, revision_id: &str) -> anyhow::Result<StoredSkillRevision> {
        let observed = self.staging_record(revision_id).await?;
        let _revision_guard =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::PromoteAfterLock)
            .await;
        self.paths.verify_identity()?;
        let mut transition = TransitionState::new("promotion");
        let staged = PathBuf::from(&record.storage_path);
        ensure_exact_path(&staged, &self.paths.staging.join(revision_id), "staging")?;
        ensure_directory_contained(&self.paths.staging, &staged, "staging").await?;
        let staged_directory =
            open_prepared_directory(self.paths.staging_identity(), Path::new(revision_id)).await?;
        let final_metadata = self.final_metadata(&record, &staged_directory).await?;
        let package_root = self
            .paths
            .managed
            .join(record.package_id.as_str())
            .join("revisions");
        let incoming_root = self.paths.managed.join(".incoming");
        ensure_directory_contained(&self.paths.managed, &self.paths.managed, "managed").await?;
        let package_relative = PathBuf::from(record.package_id.as_str()).join("revisions");
        ensure_prepared_directory(self.paths.managed_identity(), &package_relative).await?;
        ensure_prepared_directory(self.paths.managed_identity(), Path::new(".incoming")).await?;
        ensure_directory_contained(&self.paths.managed, &package_root, "managed").await?;
        ensure_directory_contained(&self.paths.managed, &incoming_root, "managed").await?;
        let incoming_name = format!("{revision_id}-{}", uuid::Uuid::new_v4());
        let incoming_relative = PathBuf::from(".incoming").join(&incoming_name);
        let destination = package_root.join(revision_id);
        let destination_relative = package_relative.join(revision_id);
        let destination_storage = storage_path(&destination)?;

        let incoming_directory =
            reserve_opened_prepared_directory(self.paths.managed_identity(), &incoming_relative)
                .await?;
        if let Err(error) = copy_prepared_package_tree_into_prepared(
            &staged_directory,
            &incoming_directory,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::IncomingCopyFile,
        )
        .await
        {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming_directory)
                .await;
        }
        transition.advance(TransitionPhase::IncomingCopied);
        let incoming_hash = match opened_package_snapshot(
            &incoming_directory,
            self.limits.package_limits(),
        )
        .await
        {
            Ok(snapshot) => snapshot.content_hash,
            Err(error) => {
                return self
                    .cleanup_incoming_error(transition.context(error), &incoming_directory)
                    .await;
            }
        };
        if incoming_hash != final_metadata.content_hash {
            return self
                .cleanup_incoming_error(
                    transition.context(anyhow::anyhow!("staging copy hash mismatch")),
                    &incoming_directory,
                )
                .await;
        }

        self.faults
            .checkpoint(StoreFaultPoint::PromoteBeforeDestinationCommit)
            .await;
        self.paths.verify_identity()?;
        let reserve = async {
            self.faults.check(StoreFaultPoint::PromoteIncomingRename)?;
            let destination_directory = reserve_opened_prepared_directory(
                self.paths.managed_identity(),
                &destination_relative,
            )
            .await?;
            let copy = copy_prepared_package_tree_into_prepared(
                &incoming_directory,
                &destination_directory,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await;
            match copy {
                Ok(()) => Ok(destination_directory),
                Err(error) => {
                    let cleanup = self
                        .cleanup_failed_promotion_destination(&destination_directory)
                        .await;
                    Err(combine_operation_errors(
                        error,
                        [("destination reservation cleanup", cleanup)],
                    ))
                }
            }
        }
        .await;
        let destination_directory = match reserve {
            Ok(destination_directory) => destination_directory,
            Err(error) => {
                return self
                    .cleanup_incoming_error(transition.context(error), &incoming_directory)
                    .await;
            }
        };
        transition.advance(TransitionPhase::DestinationReserved);
        if let Err(error) = remove_opened_prepared_tree(&incoming_directory).await {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination_directory)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        let readonly = async {
            self.faults
                .checkpoint(StoreFaultPoint::ManagedReadonlyBeforeApply)
                .await;
            destination_directory.verify()?;
            self.faults.check(StoreFaultPoint::ManagedReadonly)?;
            make_tree_readonly(&destination_directory, self.limits.package_limits()).await
        }
        .await;
        if let Err(error) = readonly {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination_directory)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        transition.advance(TransitionPhase::PermissionsApplied);
        if let Err(error) = self.faults.check(StoreFaultPoint::PromoteStagingRename) {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination_directory)
                .await;
            if let Err(cleanup_error) = &cleanup {
                self.record_maintenance_issue(
                    revision_id,
                    "promotion_destination_cleanup",
                    &destination,
                    cleanup_error,
                );
            }
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
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
                .promote_revision_record_with_metadata_cas(
                    revision_id,
                    SkillRevisionExpectation::from(&record),
                    promotion,
                )
                .await
                .context("promotion transition failed")
        }
        .await;
        let promoted = match database_result {
            Ok(record) => record,
            Err(error) => {
                let cleanup = self
                    .cleanup_failed_promotion_destination(&destination_directory)
                    .await;
                return match cleanup {
                    Ok(()) => Err(transition.context(error)),
                    Err(cleanup_error) => {
                        let isolation = self
                            .isolate_promotion_residue(revision_id, &destination_directory)
                            .await;
                        match isolation {
                            Ok(isolated) => {
                                self.record_maintenance_issue(
                                    revision_id,
                                    "promotion_destination_isolated",
                                    &isolated,
                                    &cleanup_error,
                                );
                                Err(combine_operation_errors(
                                    transition.context(error),
                                    [("destination cleanup", Err(cleanup_error))],
                                ))
                            }
                            Err(isolation_error) => Err(combine_operation_errors(
                                transition.context(error),
                                [
                                    ("destination cleanup", Err(cleanup_error)),
                                    ("destination maintenance isolation", Err(isolation_error)),
                                ],
                            )),
                        }
                    }
                };
            }
        };
        transition.advance(TransitionPhase::DatabaseCommitted);
        let mut issues = Vec::new();
        if let Err(error) = self.cleanup_promoted_source(&staged_directory).await {
            issues.push(self.record_maintenance_issue(
                revision_id,
                "promotion_source_cleanup",
                &staged,
                &error,
            ));
        }
        transition.advance(TransitionPhase::SourceCleanupAttempted);
        Ok(stored_revision(promoted, destination, issues))
    }

    pub async fn quarantine_revision(
        &self,
        revision_id: &str,
        reason: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        let observed = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("skill revision not found: {revision_id}"))?;
        if observed.status == SkillRevisionStatus::Quarantined {
            anyhow::bail!("revision is already quarantined: {revision_id}");
        }
        let _revision_guard =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::QuarantineAfterLock)
            .await;
        self.paths.verify_identity()?;
        let mut transition = TransitionState::new("quarantine");
        let source = self.expected_revision_path(&record)?;
        let (source_root, source_identity, source_relative) = match record.status {
            SkillRevisionStatus::Staging => (
                &self.paths.staging,
                self.paths.staging_identity(),
                PathBuf::from(revision_id),
            ),
            SkillRevisionStatus::Managed => (
                &self.paths.managed,
                self.paths.managed_identity(),
                PathBuf::from(record.package_id.as_str())
                    .join("revisions")
                    .join(revision_id),
            ),
            SkillRevisionStatus::Quarantined => unreachable!("quarantined revisions rejected"),
        };
        ensure_directory_contained(source_root, &source, "revision").await?;
        let source_directory = open_prepared_directory(source_identity, &source_relative).await?;
        measure_package_tree(&source, self.limits.package_limits(), None).await?;
        let replacement_metadata = self
            .actual_quarantine_metadata(&record, &source_directory, reason)
            .await?;
        let quarantine_incoming_root = self.paths.quarantine.join(".incoming");
        ensure_directory_contained(&self.paths.quarantine, &self.paths.quarantine, "quarantine")
            .await?;
        ensure_prepared_directory(self.paths.quarantine_identity(), Path::new(".incoming")).await?;
        ensure_directory_contained(
            &self.paths.quarantine,
            &quarantine_incoming_root,
            "quarantine",
        )
        .await?;
        let incoming_name = format!("{revision_id}-{}", uuid::Uuid::new_v4());
        let incoming_relative = PathBuf::from(".incoming").join(&incoming_name);
        let destination = self.paths.quarantine.join(revision_id);
        let incoming_directory =
            reserve_opened_prepared_directory(self.paths.quarantine_identity(), &incoming_relative)
                .await?;
        if let Err(error) = copy_prepared_package_tree_into_prepared(
            &source_directory,
            &incoming_directory,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::QuarantineCopyFile,
        )
        .await
        {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming_directory)
                .await;
        }
        transition.advance(TransitionPhase::IncomingCopied);
        let reserve = async {
            self.faults
                .check(StoreFaultPoint::QuarantineIncomingRename)?;
            let destination_directory = reserve_opened_prepared_directory(
                self.paths.quarantine_identity(),
                Path::new(revision_id),
            )
            .await?;
            let copy = copy_prepared_package_tree_into_prepared(
                &incoming_directory,
                &destination_directory,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::QuarantineCopyFile,
            )
            .await;
            match copy {
                Ok(()) => Ok(destination_directory),
                Err(error) => {
                    let cleanup = self
                        .cleanup_failed_quarantine_destination(&destination_directory)
                        .await;
                    Err(combine_operation_errors(
                        error,
                        [("destination reservation cleanup", cleanup)],
                    ))
                }
            }
        }
        .await;
        let destination_directory = match reserve {
            Ok(destination_directory) => destination_directory,
            Err(error) => {
                return self
                    .cleanup_incoming_error(transition.context(error), &incoming_directory)
                    .await;
            }
        };
        transition.advance(TransitionPhase::DestinationReserved);
        let incoming_cleanup = async {
            incoming_directory.verify()?;
            make_tree_writable(&incoming_directory, self.limits.package_limits()).await?;
            remove_opened_prepared_tree(&incoming_directory).await
        }
        .await;
        if let Err(error) = incoming_cleanup {
            let cleanup = self
                .cleanup_failed_quarantine_destination(&destination_directory)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        if let Err(error) = self.faults.check(StoreFaultPoint::QuarantineSourceRename) {
            let cleanup = self
                .cleanup_failed_quarantine_destination(&destination_directory)
                .await;
            if let Err(cleanup_error) = &cleanup {
                self.record_maintenance_issue(
                    revision_id,
                    "quarantine_destination_cleanup",
                    &destination,
                    cleanup_error,
                );
            }
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        let database_result = async {
            self.faults.check(StoreFaultPoint::QuarantineDatabase)?;
            self.state
                .quarantine_revision_record_cas(
                    revision_id,
                    &storage_path(&destination)?,
                    reason,
                    SkillRevisionExpectation::from(&record),
                    Some(replacement_metadata),
                )
                .await
                .context("quarantine transition failed")
        }
        .await;
        let quarantined = match database_result {
            Ok(record) => record,
            Err(error) => {
                let cleanup = self
                    .cleanup_failed_quarantine_destination(&destination_directory)
                    .await;
                if let Err(cleanup_error) = &cleanup {
                    self.record_maintenance_issue(
                        revision_id,
                        "quarantine_destination_cleanup",
                        &destination,
                        cleanup_error,
                    );
                }
                return Err(combine_operation_errors(
                    transition.context(error),
                    [("destination cleanup", cleanup)],
                ));
            }
        };
        transition.advance(TransitionPhase::DatabaseCommitted);
        let mut issues = Vec::new();
        if let Err(error) = self.cleanup_quarantined_source(&source_directory).await {
            issues.push(self.record_maintenance_issue(
                revision_id,
                "quarantine_source_cleanup",
                &source,
                &error,
            ));
        }
        transition.advance(TransitionPhase::SourceCleanupAttempted);
        Ok(stored_revision(quarantined, destination, issues))
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

    async fn revision_after_wait(
        &self,
        observed: &SkillRevisionRecord,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let current = self
            .state
            .get_revision(&observed.revision_id)
            .await?
            .with_context(|| format!("skill revision not found: {}", observed.revision_id))?;
        if current.status != observed.status || current.storage_path != observed.storage_path {
            anyhow::bail!(
                "skill revision changed while waiting for revision lock: {}",
                observed.revision_id
            );
        }
        Ok(current)
    }

    async fn refresh_staging_metadata(
        &self,
        record: &SkillRevisionRecord,
        root: &crate::skill_store_secure_roots::PreparedStoreDirectory,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let metadata = self.final_metadata(record, root).await?;
        self.state
            .refresh_staging_revision_metadata_cas(
                &record.revision_id,
                SkillRevisionExpectation::from(record),
                metadata,
            )
            .await
    }

    async fn final_metadata(
        &self,
        record: &SkillRevisionRecord,
        root: &crate::skill_store_secure_roots::PreparedStoreDirectory,
    ) -> anyhow::Result<SkillRevisionMetadata> {
        let snapshot = opened_package_snapshot(root, self.limits.package_limits()).await?;
        let loaded = snapshot.descriptor;
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
            content_hash: snapshot.content_hash,
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
}
