use crate::skill_state::SkillRevisionRecord;
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::make_tree_writable;
use crate::skill_store_operations::error_is_not_found;
use crate::skill_store_secure_roots::{
    open_prepared_directory, opened_package_snapshot, remove_opened_tree,
};
use std::path::PathBuf;

impl SkillRevisionStore {
    pub(crate) async fn delete_managed_revision_tree(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<bool> {
        self.paths.verify_identity()?;
        self.expected_revision_path(record)?;
        let relative = PathBuf::from(record.package_id.as_str())
            .join("revisions")
            .join(&record.revision_id);
        let directory =
            match open_prepared_directory(self.paths.managed_identity(), &relative).await {
                Ok(directory) => directory,
                Err(error) if error_is_not_found(&error) => return Ok(false),
                Err(error) => return Err(error),
            };
        let snapshot = opened_package_snapshot(&directory, self.limits.package_limits()).await?;
        directory.verify()?;
        if snapshot.content_hash != record.content_hash
            || snapshot.descriptor.descriptor.id != record.package_id
            || snapshot.descriptor.descriptor.version.to_string() != record.version
            || serde_json::to_value(&snapshot.descriptor.descriptor)? != record.descriptor_json
        {
            anyhow::bail!("managed cleanup tree does not match its revision record");
        }
        self.faults
            .checkpoint(StoreFaultPoint::CleanupBeforeTreeDelete)
            .await;
        self.faults
            .check(StoreFaultPoint::CleanupBeforeTreeDelete)?;
        make_tree_writable(&directory, self.limits.package_limits()).await?;
        remove_opened_tree(&directory).await?;
        self.faults.check(StoreFaultPoint::CleanupAfterTreeDelete)?;
        Ok(true)
    }
}
