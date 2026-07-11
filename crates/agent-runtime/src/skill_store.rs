use crate::skill_package::{SkillPackageDescriptor, SkillPackageId};
use crate::skill_source::{canonical_relative_path, hash_package_tree};
use crate::skill_state::{
    NewSkillRevision, SkillRevisionMetadata, SkillRevisionPromotion, SkillRevisionRecord,
    SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs::{
    PackageLimits, atomic_replace_file, copy_package_tree, ensure_directory_contained,
    ensure_safe_write_parent, make_tree_readonly, make_tree_writable, measure_package_tree,
    read_optional_regular_file, remove_created_directories, remove_regular_file_nofollow,
    reserve_and_copy_package_tree,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

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
    fn package_limits(self) -> PackageLimits {
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
    revision_locks: RevisionLockRegistry,
    maintenance_issues: Arc<RwLock<Vec<SkillStoreMaintenanceIssue>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransitionPhase {
    Initial,
    IncomingCopied,
    DestinationReserved,
    PermissionsApplied,
    DatabaseCommitted,
    SourceCleanupAttempted,
}

#[derive(Debug)]
struct TransitionState {
    operation: &'static str,
    phase: TransitionPhase,
}

impl TransitionState {
    fn new(operation: &'static str) -> Self {
        Self {
            operation,
            phase: TransitionPhase::Initial,
        }
    }

    fn advance(&mut self, phase: TransitionPhase) {
        self.phase = phase;
    }

    fn context(&self, error: anyhow::Error) -> anyhow::Error {
        error.context(format!(
            "{} transition failed in {:?} phase",
            self.operation, self.phase
        ))
    }
}

#[derive(Clone, Default)]
struct RevisionLockRegistry {
    locks: Arc<TokioMutex<HashMap<String, Weak<TokioMutex<()>>>>>,
}

impl RevisionLockRegistry {
    async fn acquire(&self, revision_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            match locks.get(revision_id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(TokioMutex::new(()));
                    locks.insert(revision_id.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
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
            revision_locks: RevisionLockRegistry::default(),
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
            revision_locks: RevisionLockRegistry::default(),
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

    pub async fn create_staging_revision(
        &self,
        source: &Path,
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        ensure_directory_contained(&self.paths.staging, &self.paths.staging, "staging").await?;
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
            make_tree_writable(&destination, self.limits.package_limits()).await?;
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
        let _revision_guard = self.revision_locks.acquire(revision_id).await;
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
            let restore = match previous {
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
        Ok(())
    }

    pub async fn promote_revision(&self, revision_id: &str) -> anyhow::Result<StoredSkillRevision> {
        let observed = self.staging_record(revision_id).await?;
        let _revision_guard = self.revision_locks.acquire(revision_id).await;
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
        tokio::fs::create_dir_all(&package_root).await?;
        tokio::fs::create_dir_all(&incoming_root).await?;
        ensure_directory_contained(&self.paths.managed, &package_root, "managed").await?;
        ensure_directory_contained(&self.paths.managed, &incoming_root, "managed").await?;
        let incoming = incoming_root.join(format!("{revision_id}-{}", uuid::Uuid::new_v4()));
        let destination = package_root.join(revision_id);
        let destination_storage = storage_path(&destination)?;
        reject_path_exists(&destination).await?;

        if let Err(error) = copy_package_tree(
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
        let incoming_hash = match hash_package_tree(&incoming).await {
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
            reserve_and_copy_package_tree(
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
                .promote_revision_record_with_metadata(revision_id, promotion)
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
        let _revision_guard = self.revision_locks.acquire(revision_id).await;
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
        tokio::fs::create_dir_all(&quarantine_incoming_root).await?;
        ensure_directory_contained(
            &self.paths.quarantine,
            &quarantine_incoming_root,
            "quarantine",
        )
        .await?;
        let incoming =
            quarantine_incoming_root.join(format!("{revision_id}-{}", uuid::Uuid::new_v4()));
        let destination = self.paths.quarantine.join(revision_id);
        reject_path_exists(&destination).await?;
        if let Err(error) = copy_package_tree(
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
            reserve_and_copy_package_tree(
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
                .quarantine_revision_record(revision_id, &storage_path(&destination)?, reason)
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

fn stored_revision(
    record: SkillRevisionRecord,
    path: PathBuf,
    maintenance_issues: Vec<SkillStoreMaintenanceIssue>,
) -> StoredSkillRevision {
    StoredSkillRevision {
        revision_id: record.revision_id,
        package_id: record.package_id,
        path,
        content_hash: record.content_hash,
        maintenance_issues,
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

async fn cleanup_created_directories_error(
    error: anyhow::Error,
    created_directories: &[PathBuf],
) -> anyhow::Result<()> {
    match remove_created_directories(created_directories).await {
        Ok(()) => Err(error),
        Err(cleanup) => Err(with_compensation(error, cleanup)),
    }
}

fn combine_operation_errors<const N: usize>(
    primary: anyhow::Error,
    compensations: [(&str, anyhow::Result<()>); N],
) -> anyhow::Error {
    let failures = compensations
        .into_iter()
        .filter_map(|(label, result)| result.err().map(|error| format!("{label}: {error:#}")))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        primary
    } else {
        primary.context(format!("compensation failed: {}", failures.join("; ")))
    }
}

fn with_compensation(primary: anyhow::Error, compensation: anyhow::Error) -> anyhow::Error {
    primary.context(format!("compensation failed: {compensation:#}"))
}
