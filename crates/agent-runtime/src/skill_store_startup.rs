use crate::skill_package::SkillPackageId;
use crate::skill_state::{SkillRevisionRecord, SkillRevisionStatus};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_fs::make_tree_writable;
use crate::skill_store_locks::acquire_revision_lock;
use crate::skill_store_operations::error_is_not_found;
use crate::skill_store_secure_roots::{
    PreparedStoreDirectory, list_opened_child_directories, list_opened_root_directories,
    open_prepared_directory, opened_package_snapshot, remove_opened_tree,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryTreeArea {
    Staging,
    Quarantine,
    Managed,
}

#[derive(Clone, Debug)]
pub(crate) struct RecoveryTreeEntry {
    pub(crate) area: RecoveryTreeArea,
    pub(crate) package_id: Option<SkillPackageId>,
    pub(crate) name: String,
    pub(crate) directory: PreparedStoreDirectory,
}

impl SkillRevisionStore {
    pub(crate) async fn enumerate_recovery_trees(&self) -> anyhow::Result<Vec<RecoveryTreeEntry>> {
        self.paths.verify_identity()?;
        let limit = usize::try_from(self.limits.max_directories)?;
        let mut result = Vec::new();
        for child in list_opened_root_directories(self.paths.staging_identity(), limit).await? {
            result.push(RecoveryTreeEntry {
                area: RecoveryTreeArea::Staging,
                package_id: None,
                name: child.name,
                directory: child.directory,
            });
        }
        for child in list_opened_root_directories(self.paths.quarantine_identity(), limit).await? {
            result.push(RecoveryTreeEntry {
                area: RecoveryTreeArea::Quarantine,
                package_id: None,
                name: child.name,
                directory: child.directory,
            });
        }
        for package in list_opened_root_directories(self.paths.managed_identity(), limit).await? {
            if package.name == ".locks" {
                continue;
            }
            let package_id = SkillPackageId::parse(&package.name).ok();
            let revisions = match open_prepared_directory(
                self.paths.managed_identity(),
                &PathBuf::from(&package.name).join("revisions"),
            )
            .await
            {
                Ok(directory) => directory,
                Err(error) if error_is_not_found(&error) => continue,
                Err(error) => return Err(error),
            };
            for child in list_opened_child_directories(&revisions, limit).await? {
                result.push(RecoveryTreeEntry {
                    area: RecoveryTreeArea::Managed,
                    package_id: package_id.clone(),
                    name: child.name,
                    directory: child.directory,
                });
            }
        }
        result.sort_by(|left, right| {
            format!("{:?}", left.area)
                .cmp(&format!("{:?}", right.area))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(result)
    }

    pub(crate) async fn revision_tree_exists(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<bool> {
        let (root, relative) = match record.status {
            SkillRevisionStatus::Staging => {
                let (_, relative) = self.staging_revision_path(record)?;
                (self.paths.staging_identity(), relative)
            }
            SkillRevisionStatus::Managed => (
                self.paths.managed_identity(),
                PathBuf::from(record.package_id.as_str())
                    .join("revisions")
                    .join(&record.revision_id),
            ),
            SkillRevisionStatus::Quarantined => {
                let relative = Path::new(&record.storage_path)
                    .strip_prefix(&self.paths.quarantine)?
                    .to_path_buf();
                crate::skill_source::canonical_relative_path(&relative)?;
                (self.paths.quarantine_identity(), relative)
            }
        };
        match open_prepared_directory(root, &relative).await {
            Ok(directory) => {
                directory.verify()?;
                Ok(true)
            }
            Err(error) if error_is_not_found(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn cleanup_incomplete_promotion_candidate(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<()> {
        if record.status != SkillRevisionStatus::Staging {
            anyhow::bail!("incomplete promotion cleanup requires a staging row");
        }
        let _revision_lock =
            acquire_revision_lock(&self.paths.identity, &record.revision_id, &self.faults).await?;
        let current = self
            .state
            .get_revision(&record.revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("promotion candidate row disappeared"))?;
        if current != *record {
            anyhow::bail!("promotion candidate row changed during recovery");
        }
        let relative = PathBuf::from(record.package_id.as_str())
            .join("revisions")
            .join(&record.revision_id);
        let directory = open_prepared_directory(self.paths.managed_identity(), &relative).await?;
        let snapshot = opened_package_snapshot(&directory, self.limits.package_limits()).await?;
        directory.verify()?;
        if snapshot.content_hash != record.content_hash
            || snapshot.descriptor.descriptor.id != record.package_id
            || snapshot.descriptor.descriptor.version.to_string() != record.version
            || serde_json::to_value(&snapshot.descriptor.descriptor)? != record.descriptor_json
        {
            anyhow::bail!("promotion candidate bytes do not match the staging row");
        }
        make_tree_writable(&directory, self.limits.package_limits()).await?;
        remove_opened_tree(&directory).await
    }
}
