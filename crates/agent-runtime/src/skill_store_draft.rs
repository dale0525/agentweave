use crate::skill_source::canonical_relative_path;
use crate::skill_state::{SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionRecord};
use crate::skill_store::{SkillRevisionStore, StagingSkillFile};
use crate::skill_store_atomic_write::atomic_replace_file;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::{
    copy_prepared_package_tree_into_prepared, ensure_directory_contained, make_tree_writable,
};
use crate::skill_store_locks::{RevisionOperationGuard, acquire_revision_lock};
use crate::skill_store_operations::{error_is_not_found, storage_path};
use crate::skill_store_prepared_fs::open_regular_file;
use crate::skill_store_public_types::SkillStoreBoundaryError;
use crate::skill_store_secure_roots::{
    ensure_opened_child_directory, open_prepared_directory, opened_package_snapshot,
    remove_opened_tree, reserve_opened_directory,
};
use serde_json::json;
use std::path::{Path, PathBuf};

pub(crate) struct StagingPackageSnapshot {
    pub record: SkillRevisionRecord,
    pub descriptor: crate::skill_package::LoadedPackageDescriptor,
    pub content_hash: String,
    pub runtime_manifest: Option<Vec<u8>>,
    pub instructions_file: Option<Vec<u8>>,
}

pub(crate) struct LockedStagingPackageSnapshot {
    pub(crate) snapshot: StagingPackageSnapshot,
    _revision_lock: RevisionOperationGuard,
}

impl SkillRevisionStore {
    pub(crate) async fn lock_staging_revision_snapshot(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<LockedStagingPackageSnapshot> {
        let revision_lock =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.state.get_revision(revision_id).await?.ok_or_else(|| {
            SkillStoreBoundaryError::NotFound(anyhow::anyhow!("skill revision not found"))
        })?;
        if record.status != crate::skill_state::SkillRevisionStatus::Staging {
            return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                "revision is not an editable staging revision"
            ))
            .into());
        }
        let snapshot = self.snapshot_inactive_record(record).await?;
        Ok(LockedStagingPackageSnapshot {
            snapshot,
            _revision_lock: revision_lock,
        })
    }

    pub(crate) async fn snapshot_staging_revision(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<StagingPackageSnapshot> {
        let snapshot = self.snapshot_inactive_revision(revision_id).await?;
        if snapshot.record.status != crate::skill_state::SkillRevisionStatus::Staging {
            return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                "revision is not an editable staging revision"
            ))
            .into());
        }
        Ok(snapshot)
    }

    pub(crate) async fn snapshot_inactive_revision(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<StagingPackageSnapshot> {
        let observed = self.state.get_revision(revision_id).await?.ok_or_else(|| {
            SkillStoreBoundaryError::NotFound(anyhow::anyhow!("skill revision not found"))
        })?;
        let _revision_guard =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.state.get_revision(revision_id).await?.ok_or_else(|| {
            SkillStoreBoundaryError::NotFound(anyhow::anyhow!("skill revision not found"))
        })?;
        if record != observed {
            return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                "skill revision changed while waiting for revision lock"
            ))
            .into());
        }
        self.snapshot_inactive_record(record).await
    }

    async fn snapshot_inactive_record(
        &self,
        record: SkillRevisionRecord,
    ) -> anyhow::Result<StagingPackageSnapshot> {
        self.paths.verify_identity()?;
        let revision_id = &record.revision_id;
        let (identity, relative) = match record.status {
            crate::skill_state::SkillRevisionStatus::Staging => {
                let (_, relative) = self.staging_revision_path(&record)?;
                (self.paths.staging_identity(), relative)
            }
            crate::skill_state::SkillRevisionStatus::Quarantined => {
                let expected = self.paths.quarantine.join(revision_id);
                if Path::new(&record.storage_path) != expected {
                    anyhow::bail!("quarantined revision storage binding is invalid");
                }
                (self.paths.quarantine_identity(), PathBuf::from(revision_id))
            }
            crate::skill_state::SkillRevisionStatus::Managed => {
                return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                    "managed revision is not an inactive draft"
                ))
                .into());
            }
        };
        let directory = open_prepared_directory(identity, &relative).await?;
        let snapshot = opened_package_snapshot(&directory, self.limits.package_limits()).await?;
        directory.verify()?;
        if snapshot.content_hash != record.content_hash {
            anyhow::bail!("staging revision bytes do not match recorded content hash");
        }
        Ok(StagingPackageSnapshot {
            record,
            descriptor: snapshot.descriptor,
            content_hash: snapshot.content_hash,
            runtime_manifest: snapshot.runtime_manifest,
            instructions_file: snapshot.instructions_file,
        })
    }

    pub async fn write_staging_file(
        &self,
        revision_id: &str,
        relative_path: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        self.write_staging_files(
            revision_id,
            vec![StagingSkillFile {
                path: relative_path.to_path_buf(),
                bytes: bytes.to_vec(),
            }],
        )
        .await
    }

    pub async fn write_staging_files(
        &self,
        revision_id: &str,
        files: Vec<StagingSkillFile>,
    ) -> anyhow::Result<()> {
        self.validate_authored_input(&files)?;
        let store = self.clone();
        let revision_id = revision_id.to_string();
        tokio::spawn(async move { store.write_staging_files_inner(&revision_id, files).await })
            .await
            .map_err(|error| anyhow::anyhow!("staging update task failed: {error}"))?
    }

    async fn write_staging_files_inner(
        &self,
        revision_id: &str,
        files: Vec<StagingSkillFile>,
    ) -> anyhow::Result<()> {
        for file in &files {
            canonical_relative_path(&file.path)?;
        }
        let observed = self.staging_record(revision_id).await?;
        let _revision_guard =
            acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        let record = self.revision_after_wait(&observed).await?;
        self.faults
            .checkpoint(StoreFaultPoint::WriteAfterLock)
            .await;
        self.paths.verify_identity()?;
        let (root, source_relative) = self.staging_revision_path(&record)?;
        ensure_directory_contained(&self.paths.staging, &root, "staging").await?;
        let prepared_root =
            open_prepared_directory(self.paths.staging_identity(), &source_relative).await?;
        let candidate_relative =
            PathBuf::from(format!("{revision_id}.candidate.{}", uuid::Uuid::new_v4()));
        let candidate = self.paths.staging.join(&candidate_relative);
        let candidate_directory =
            reserve_opened_directory(self.paths.staging_identity(), &candidate_relative).await?;
        let result = async {
            copy_prepared_package_tree_into_prepared(
                &prepared_root,
                &candidate_directory,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::StagingCopyFile,
            )
            .await?;
            make_tree_writable(&candidate_directory, self.limits.package_limits()).await?;
            for file in &files {
                if let Some(parent) = file.path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    ensure_opened_child_directory(&candidate_directory, parent).await?;
                }
                let mode = match open_regular_file(&candidate_directory, &file.path).await {
                    Ok((opened, _, mode)) => {
                        drop(opened);
                        mode
                    }
                    Err(error) if error_is_not_found(&error) => 0o644,
                    Err(error) => return Err(error),
                };
                if let Err(failure) = atomic_replace_file(
                    &candidate_directory,
                    &file.path,
                    &file.bytes,
                    mode,
                    &self.faults,
                )
                .await
                {
                    if let Some(temp_path) = &failure.temp_path {
                        self.record_maintenance_issue(
                            revision_id,
                            "staging_write_temp_cleanup",
                            temp_path,
                            &anyhow::anyhow!("temporary file cleanup failed"),
                        );
                    }
                    return Err(failure.into_error());
                }
            }
            let snapshot =
                opened_package_snapshot(&candidate_directory, self.limits.package_limits())
                    .await
                    .map_err(SkillStoreBoundaryError::InvalidInput)?;
            let descriptor = snapshot.descriptor.descriptor;
            descriptor
                .validate()
                .map_err(SkillStoreBoundaryError::InvalidInput)?;
            if descriptor.id != record.package_id {
                return Err(SkillStoreBoundaryError::InvalidInput(anyhow::anyhow!(
                    "draft package id changed during update"
                ))
                .into());
            }
            let recorded_descriptor: crate::skill_package::SkillPackageDescriptor =
                serde_json::from_value(record.descriptor_json.clone())?;
            if descriptor.kind != recorded_descriptor.kind {
                return Err(SkillStoreBoundaryError::InvalidInput(anyhow::anyhow!(
                    "draft package kind cannot be changed during update"
                ))
                .into());
            }
            let metadata = SkillRevisionMetadata {
                version: descriptor.version.to_string(),
                content_hash: snapshot.content_hash,
                descriptor_json: serde_json::to_value(descriptor)?,
                validation_json: json!({"status": "pending"}),
            };
            self.faults
                .checkpoint(StoreFaultPoint::WriteBeforeMetadataCommit)
                .await;
            self.state
                .replace_staging_revision_cas(
                    revision_id,
                    SkillRevisionExpectation::from(&record),
                    &storage_path(&candidate)?,
                    metadata,
                )
                .await?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            return self
                .cleanup_staging_candidate_error(revision_id, error, &candidate_directory)
                .await;
        }
        if let Err(error) = remove_opened_tree(&prepared_root).await {
            self.record_maintenance_issue(
                revision_id,
                "staging_write_previous_cleanup",
                &root,
                &error,
            );
        }
        Ok(())
    }
}
