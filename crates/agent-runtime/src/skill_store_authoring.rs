use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_source::canonical_relative_path;
use crate::skill_state::{NewSkillRevision, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreLimits, StagingSkillFile, StoredSkillRevision,
};
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::ensure_directory_contained;
use crate::skill_store_operations::{storage_path, stored_revision, with_compensation};
use crate::skill_store_prepared_fs::create_regular_file;
use crate::skill_store_secure_roots::{
    ensure_opened_child_directory, opened_package_snapshot, remove_opened_tree,
    reserve_opened_directory,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

impl SkillRevisionStore {
    pub(crate) fn validate_authored_input(&self, files: &[StagingSkillFile]) -> anyhow::Result<()> {
        validate_authored_files(files, self.limits)
    }

    pub async fn create_authored_staging_revision(
        &self,
        expected_package_id: &SkillPackageId,
        expected_kind: SkillPackageKind,
        files: &[StagingSkillFile],
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        self.validate_authored_input(files)?;
        let store = self.clone();
        let expected_package_id = expected_package_id.clone();
        let files = files.to_vec();
        let actor_id = actor_id.to_string();
        tokio::spawn(async move {
            store
                .create_authored_staging_revision_inner(
                    expected_package_id,
                    expected_kind,
                    files,
                    actor_id,
                )
                .await
        })
        .await
        .map_err(|error| anyhow::anyhow!("authored staging operation task failed: {error}"))?
    }

    async fn create_authored_staging_revision_inner(
        &self,
        expected_package_id: SkillPackageId,
        expected_kind: SkillPackageKind,
        files: Vec<StagingSkillFile>,
        actor_id: String,
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
            let reserved_directory =
                reserve_opened_directory(self.paths.staging_identity(), Path::new(&revision_id))
                    .await?;
            reserved = Some(reserved_directory.clone());
            self.faults
                .checkpoint(StoreFaultPoint::StagingAuthorAfterReservation)
                .await;
            self.paths.verify_identity()?;
            for file in &files {
                self.faults.check(StoreFaultPoint::StagingAuthorFile)?;
                if let Some(parent) = file.path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    ensure_opened_child_directory(&reserved_directory, parent).await?;
                }
                let mut destination_file =
                    create_regular_file(&reserved_directory, &file.path, 0o600).await?;
                destination_file.write_all(&file.bytes).await?;
                destination_file.flush().await?;
            }
            let snapshot =
                opened_package_snapshot(&reserved_directory, self.limits.package_limits()).await?;
            snapshot.descriptor.descriptor.validate()?;
            if snapshot.descriptor.descriptor.id != expected_package_id {
                anyhow::bail!("draft package id changed during creation");
            }
            if snapshot.descriptor.descriptor.kind != expected_kind {
                anyhow::bail!("draft package kind changed during creation");
            }
            let content_hash = snapshot.content_hash;
            let descriptor = snapshot.descriptor.descriptor;
            let record = self
                .state
                .create_staging_revision_record(
                    &revision_id,
                    NewSkillRevision {
                        package_id: descriptor.id.clone(),
                        version: descriptor.version.to_string(),
                        content_hash: content_hash.clone(),
                        storage_path: storage_path(&destination)?,
                        descriptor_json: serde_json::to_value(&descriptor)?,
                        validation_json: json!({"status": "pending"}),
                        created_by: actor_id,
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
                match remove_opened_tree(reserved.as_ref().expect("authoring reservation recorded"))
                    .await
                {
                    Ok(()) => Err(error),
                    Err(compensation) => Err(with_compensation(error, compensation)),
                }
            }
        }
    }
}

fn validate_authored_files(
    files: &[StagingSkillFile],
    limits: SkillStoreLimits,
) -> anyhow::Result<()> {
    if files.is_empty() {
        anyhow::bail!("authored skill package has no files");
    }
    let file_count = u64::try_from(files.len())?;
    if file_count > limits.max_files || file_count > limits.max_entries {
        anyhow::bail!("authored skill package has too many files");
    }
    let mut paths = BTreeSet::new();
    let mut directories = BTreeSet::<PathBuf>::new();
    let mut package_bytes = 0_u64;
    for file in files {
        canonical_relative_path(&file.path)?;
        if !paths.insert(file.path.clone()) {
            anyhow::bail!("duplicate authored skill path: {}", file.path.display());
        }
        let path_bytes = u64::try_from(file.path.to_string_lossy().len())?;
        if path_bytes > limits.max_relative_path_bytes {
            anyhow::bail!("authored skill path exceeds relative path limit");
        }
        let depth = u64::try_from(file.path.components().count())?;
        if depth > limits.max_depth {
            anyhow::bail!("authored skill path exceeds depth limit");
        }
        collect_parent_directories(&file.path, &mut directories);
        let file_bytes = u64::try_from(file.bytes.len())?;
        if file_bytes > limits.max_file_bytes {
            anyhow::bail!("authored skill file exceeds file size limit");
        }
        package_bytes = package_bytes
            .checked_add(file_bytes)
            .ok_or_else(|| anyhow::anyhow!("authored skill package size overflow"))?;
    }
    if package_bytes > limits.max_package_bytes {
        anyhow::bail!("authored skill package exceeds package size limit");
    }
    if u64::try_from(directories.len())? > limits.max_directories {
        anyhow::bail!("authored skill package has too many directories");
    }
    let entry_count = file_count
        .checked_add(u64::try_from(directories.len())?)
        .ok_or_else(|| anyhow::anyhow!("authored skill entry count overflow"))?;
    if entry_count > limits.max_entries {
        anyhow::bail!("authored skill package has too many entries");
    }
    Ok(())
}

fn collect_parent_directories(path: &Path, directories: &mut BTreeSet<PathBuf>) {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent.as_os_str().is_empty() {
            break;
        }
        directories.insert(parent.to_path_buf());
        current = parent.parent();
    }
}
