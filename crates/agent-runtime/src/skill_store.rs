use crate::skill_package::SkillPackageId;
use crate::skill_source::canonical_relative_path;
use crate::skill_state::{
    NewSkillRevision, SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion,
    SkillRevisionRecord, SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs::{
    PackageLimits, atomic_replace_file, copy_package_tree_into_reserved,
    ensure_directory_contained, ensure_safe_write_parent, make_tree_readonly, make_tree_writable,
    measure_package_tree, read_optional_regular_file, remove_created_directories,
    remove_regular_file_nofollow,
};
use crate::skill_store_locks::{SkillStoreIdentity, acquire_revision_lock};
use crate::skill_store_operations::{
    TransitionPhase, TransitionState, cleanup_created_directories_error, combine_operation_errors,
    ensure_exact_path, remove_tree_if_exists, storage_path, stored_revision, with_compensation,
};
use crate::skill_store_secure_fs::{
    ensure_store_directory, reserve_store_directory, secure_package_hash, secure_package_snapshot,
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
    identity: SkillStoreIdentity,
}

impl SkillStorePaths {
    pub async fn prepare(app_data_root: &Path, cache_root: &Path) -> anyhow::Result<Self> {
        let managed = app_data_root.join("managed-skills");
        let staging = cache_root.join("skill-staging");
        let quarantine = app_data_root.join("skill-quarantine");
        for path in [&managed, &staging, &quarantine] {
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
        let identity = SkillStoreIdentity::new(
            tokio::fs::canonicalize(&managed).await?,
            tokio::fs::canonicalize(&staging).await?,
            tokio::fs::canonicalize(&quarantine).await?,
        );
        Ok(Self {
            managed,
            staging,
            quarantine,
            identity,
        })
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
    paths: SkillStorePaths,
    state: SkillStateStore,
    limits: SkillStoreLimits,
    faults: StoreFaults,
    maintenance_issues: Arc<RwLock<Vec<SkillStoreMaintenanceIssue>>>,
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

    pub async fn create_staging_revision(
        &self,
        source: &Path,
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        ensure_directory_contained(&self.paths.staging, &self.paths.staging, "staging").await?;
        let revision_id = SkillStateStore::allocate_revision_id();
        let destination = self.paths.staging.join(&revision_id);
        let result = async {
            reserve_store_directory(&self.paths.staging, Path::new(&revision_id)).await?;
            copy_package_tree_into_reserved(
                source,
                &destination,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            make_tree_writable(&destination, self.limits.package_limits()).await?;
            let snapshot =
                secure_package_snapshot(&destination, self.limits.package_limits()).await?;
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
        let observed = self.staging_record(revision_id).await?;
        let _revision_guard = acquire_revision_lock(&self.paths.identity, revision_id).await;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::WriteAfterLock)
            .await;
        let root = PathBuf::from(&record.storage_path);
        let expected = self.paths.staging.join(revision_id);
        ensure_exact_path(&root, &expected, "staging")?;
        ensure_directory_contained(&self.paths.staging, &root, "staging").await?;
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
        if let Err(error) =
            atomic_replace_file(&root, relative_path, bytes, mode, &self.faults).await
        {
            let restore = match &previous {
                Some(previous) => {
                    atomic_replace_file(
                        &root,
                        relative_path,
                        &previous.bytes,
                        previous.mode,
                        &self.faults,
                    )
                    .await
                }
                None => remove_regular_file_nofollow(&root, relative_path).await,
            };
            let directory_cleanup = remove_created_directories(&created_directories).await;
            return Err(combine_operation_errors(
                error,
                [
                    ("file restore", restore),
                    ("parent cleanup", directory_cleanup),
                ],
            ));
        }
        let update = self.refresh_staging_metadata(&record, &root).await;
        if let Err(error) = update {
            let restore = async {
                self.faults.check(StoreFaultPoint::WriteRestore)?;
                match previous {
                    Some(previous) => {
                        atomic_replace_file(
                            &root,
                            relative_path,
                            &previous.bytes,
                            previous.mode,
                            &self.faults,
                        )
                        .await
                    }
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
                    .isolate_failed_staging_write(&record, &root, primary)
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
        let _revision_guard = acquire_revision_lock(&self.paths.identity, revision_id).await;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::PromoteAfterLock)
            .await;
        let mut transition = TransitionState::new("promotion");
        let staged = PathBuf::from(&record.storage_path);
        ensure_exact_path(&staged, &self.paths.staging.join(revision_id), "staging")?;
        ensure_directory_contained(&self.paths.staging, &staged, "staging").await?;
        let final_metadata = self.final_metadata(&record, &staged).await?;
        let package_root = self
            .paths
            .managed
            .join(record.package_id.as_str())
            .join("revisions");
        let incoming_root = self.paths.managed.join(".incoming");
        ensure_directory_contained(&self.paths.managed, &self.paths.managed, "managed").await?;
        let package_relative = PathBuf::from(record.package_id.as_str()).join("revisions");
        ensure_store_directory(&self.paths.managed, &package_relative).await?;
        ensure_store_directory(&self.paths.managed, Path::new(".incoming")).await?;
        ensure_directory_contained(&self.paths.managed, &package_root, "managed").await?;
        ensure_directory_contained(&self.paths.managed, &incoming_root, "managed").await?;
        let incoming_name = format!("{revision_id}-{}", uuid::Uuid::new_v4());
        let incoming_relative = PathBuf::from(".incoming").join(&incoming_name);
        let incoming = incoming_root.join(&incoming_name);
        let destination = package_root.join(revision_id);
        let destination_relative = package_relative.join(revision_id);
        let destination_storage = storage_path(&destination)?;

        reserve_store_directory(&self.paths.managed, &incoming_relative).await?;
        if let Err(error) = copy_package_tree_into_reserved(
            &staged,
            &incoming,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::IncomingCopyFile,
        )
        .await
        {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming)
                .await;
        }
        transition.advance(TransitionPhase::IncomingCopied);
        let incoming_hash = match secure_package_hash(&incoming, self.limits.package_limits()).await
        {
            Ok(hash) => hash,
            Err(error) => {
                return self
                    .cleanup_incoming_error(transition.context(error), &incoming)
                    .await;
            }
        };
        if incoming_hash != final_metadata.content_hash {
            return self
                .cleanup_incoming_error(
                    transition.context(anyhow::anyhow!("staging copy hash mismatch")),
                    &incoming,
                )
                .await;
        }

        self.faults
            .checkpoint(StoreFaultPoint::PromoteBeforeDestinationCommit)
            .await;
        let reserve = async {
            self.faults.check(StoreFaultPoint::PromoteIncomingRename)?;
            reserve_store_directory(&self.paths.managed, &destination_relative).await?;
            copy_package_tree_into_reserved(
                &incoming,
                &destination,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await
        }
        .await;
        if let Err(error) = reserve {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming)
                .await;
        }
        transition.advance(TransitionPhase::DestinationReserved);
        if let Err(error) = remove_tree_if_exists(&incoming).await {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        let readonly = async {
            self.faults.check(StoreFaultPoint::ManagedReadonly)?;
            make_tree_readonly(&destination, self.limits.package_limits()).await
        }
        .await;
        if let Err(error) = readonly {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        transition.advance(TransitionPhase::PermissionsApplied);
        if let Err(error) = self.faults.check(StoreFaultPoint::PromoteStagingRename) {
            let cleanup = self
                .cleanup_failed_promotion_destination(&destination)
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
                    .cleanup_failed_promotion_destination(&destination)
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
        };
        transition.advance(TransitionPhase::DatabaseCommitted);
        let mut issues = Vec::new();
        if let Err(error) = self.cleanup_promoted_source(&staged).await {
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
        let _revision_guard = acquire_revision_lock(&self.paths.identity, revision_id).await;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::QuarantineAfterLock)
            .await;
        let mut transition = TransitionState::new("quarantine");
        let source = self.expected_revision_path(&record)?;
        let source_root = match record.status {
            SkillRevisionStatus::Staging => &self.paths.staging,
            SkillRevisionStatus::Managed => &self.paths.managed,
            SkillRevisionStatus::Quarantined => unreachable!("quarantined revisions rejected"),
        };
        ensure_directory_contained(source_root, &source, "revision").await?;
        measure_package_tree(&source, self.limits.package_limits(), None).await?;
        let quarantine_incoming_root = self.paths.quarantine.join(".incoming");
        ensure_directory_contained(&self.paths.quarantine, &self.paths.quarantine, "quarantine")
            .await?;
        ensure_store_directory(&self.paths.quarantine, Path::new(".incoming")).await?;
        ensure_directory_contained(
            &self.paths.quarantine,
            &quarantine_incoming_root,
            "quarantine",
        )
        .await?;
        let incoming_name = format!("{revision_id}-{}", uuid::Uuid::new_v4());
        let incoming_relative = PathBuf::from(".incoming").join(&incoming_name);
        let incoming = quarantine_incoming_root.join(&incoming_name);
        let destination = self.paths.quarantine.join(revision_id);
        reserve_store_directory(&self.paths.quarantine, &incoming_relative).await?;
        if let Err(error) = copy_package_tree_into_reserved(
            &source,
            &incoming,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::QuarantineCopyFile,
        )
        .await
        {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming)
                .await;
        }
        transition.advance(TransitionPhase::IncomingCopied);
        let reserve = async {
            self.faults
                .check(StoreFaultPoint::QuarantineIncomingRename)?;
            reserve_store_directory(&self.paths.quarantine, Path::new(revision_id)).await?;
            copy_package_tree_into_reserved(
                &incoming,
                &destination,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::QuarantineCopyFile,
            )
            .await
        }
        .await;
        if let Err(error) = reserve {
            return self
                .cleanup_incoming_error(transition.context(error), &incoming)
                .await;
        }
        transition.advance(TransitionPhase::DestinationReserved);
        let incoming_cleanup = async {
            make_tree_writable(&incoming, self.limits.package_limits()).await?;
            remove_tree_if_exists(&incoming).await
        }
        .await;
        if let Err(error) = incoming_cleanup {
            let cleanup = self
                .cleanup_failed_quarantine_destination(&destination)
                .await;
            return Err(combine_operation_errors(
                transition.context(error),
                [("destination cleanup", cleanup)],
            ));
        }
        if let Err(error) = self.faults.check(StoreFaultPoint::QuarantineSourceRename) {
            let cleanup = self
                .cleanup_failed_quarantine_destination(&destination)
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
                    None,
                )
                .await
                .context("quarantine transition failed")
        }
        .await;
        let quarantined = match database_result {
            Ok(record) => record,
            Err(error) => {
                let cleanup = self
                    .cleanup_failed_quarantine_destination(&destination)
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
        if let Err(error) = self
            .cleanup_quarantined_source(&source, record.status)
            .await
        {
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
        let snapshot = secure_package_snapshot(root, self.limits.package_limits()).await?;
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

    async fn isolate_failed_staging_write(
        &self,
        record: &SkillRevisionRecord,
        source: &Path,
        primary: anyhow::Error,
    ) -> anyhow::Error {
        let metadata = match self.final_metadata(record, source).await {
            Ok(metadata) => metadata,
            Err(error) => return with_compensation(primary, error),
        };
        let destination = self.paths.quarantine.join(&record.revision_id);
        let reserved =
            reserve_store_directory(&self.paths.quarantine, Path::new(&record.revision_id)).await;
        let copied = match reserved {
            Ok(()) => {
                let copy = async {
                    self.faults.check(StoreFaultPoint::WriteIsolationCopy)?;
                    copy_package_tree_into_reserved(
                        source,
                        &destination,
                        self.limits.package_limits(),
                        &self.faults,
                        StoreFaultPoint::QuarantineCopyFile,
                    )
                    .await
                }
                .await;
                match copy {
                    Ok(()) => true,
                    Err(error) => {
                        let cleanup = remove_tree_if_exists(&destination).await;
                        let recovery = combine_operation_errors(
                            error,
                            [("failed isolation cleanup", cleanup)],
                        );
                        self.record_maintenance_issue(
                            &record.revision_id,
                            "staging_write_isolation_copy",
                            source,
                            &recovery,
                        );
                        false
                    }
                }
            }
            Err(error) => {
                self.record_maintenance_issue(
                    &record.revision_id,
                    "staging_write_isolation_reservation",
                    source,
                    &error,
                );
                false
            }
        };
        let authoritative = if copied { &destination } else { source };
        let database = async {
            self.faults.check(StoreFaultPoint::WriteIsolationDatabase)?;
            self.state
                .quarantine_revision_record_cas(
                    &record.revision_id,
                    &storage_path(authoritative)?,
                    "staging metadata update and file restore both failed",
                    SkillRevisionExpectation::from(record),
                    Some(metadata),
                )
                .await
        }
        .await;
        match database {
            Ok(_) if copied => {
                let cleanup = remove_tree_if_exists(source).await;
                combine_operation_errors(primary, [("staging source cleanup", cleanup)])
            }
            Ok(_) => primary,
            Err(error) => {
                let cleanup = if copied {
                    remove_tree_if_exists(&destination).await
                } else {
                    Ok(())
                };
                combine_operation_errors(
                    primary,
                    [
                        ("write isolation database transition", Err(error)),
                        ("write isolation copy cleanup", cleanup),
                    ],
                )
            }
        }
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

    async fn cleanup_failed_promotion_destination(&self, destination: &Path) -> anyhow::Result<()> {
        self.faults.check(StoreFaultPoint::PromoteRestoreRename)?;
        self.faults
            .check(StoreFaultPoint::PromoteDestinationCleanup)?;
        make_tree_writable(destination, self.limits.package_limits()).await?;
        remove_tree_if_exists(destination).await?;
        self.faults
            .check(StoreFaultPoint::PromoteDestinationCleanupAfter)
    }

    async fn cleanup_incoming_error<T>(
        &self,
        error: anyhow::Error,
        incoming: &Path,
    ) -> anyhow::Result<T> {
        let cleanup = async {
            match tokio::fs::symlink_metadata(incoming).await {
                Ok(_) => make_tree_writable(incoming, self.limits.package_limits()).await?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            remove_tree_if_exists(incoming).await
        }
        .await;
        match cleanup {
            Ok(()) => Err(error),
            Err(compensation) => Err(with_compensation(error, compensation)),
        }
    }

    async fn cleanup_failed_quarantine_destination(
        &self,
        destination: &Path,
    ) -> anyhow::Result<()> {
        self.faults
            .check(StoreFaultPoint::QuarantineRestoreRename)?;
        self.faults
            .check(StoreFaultPoint::QuarantineDestinationCleanup)?;
        make_tree_writable(destination, self.limits.package_limits()).await?;
        remove_tree_if_exists(destination).await?;
        self.faults
            .check(StoreFaultPoint::QuarantineDestinationCleanupAfter)
    }

    async fn cleanup_promoted_source(&self, source: &Path) -> anyhow::Result<()> {
        self.faults.check(StoreFaultPoint::PromoteSourceCleanup)?;
        make_tree_writable(source, self.limits.package_limits()).await?;
        remove_tree_if_exists(source).await?;
        self.faults
            .check(StoreFaultPoint::PromoteSourceCleanupAfter)
    }

    async fn cleanup_quarantined_source(
        &self,
        source: &Path,
        _status: SkillRevisionStatus,
    ) -> anyhow::Result<()> {
        self.faults
            .check(StoreFaultPoint::QuarantineSourceCleanup)?;
        make_tree_writable(source, self.limits.package_limits()).await?;
        remove_tree_if_exists(source).await?;
        self.faults
            .check(StoreFaultPoint::QuarantineSourceCleanupAfter)
    }

    fn record_maintenance_issue(
        &self,
        revision_id: &str,
        operation: &str,
        path: &Path,
        error: &anyhow::Error,
    ) -> SkillStoreMaintenanceIssue {
        let issue = SkillStoreMaintenanceIssue {
            revision_id: revision_id.to_string(),
            operation: operation.to_string(),
            path: path.to_path_buf(),
            message: format!("{error:#}"),
            recorded_at: Utc::now(),
        };
        self.maintenance_issues
            .write()
            .expect("skill store maintenance issue lock poisoned")
            .push(issue.clone());
        issue
    }
}
