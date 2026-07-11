use crate::skill_state::{SkillRevisionMetadata, SkillRevisionRecord};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::{copy_prepared_package_tree_into_prepared, make_tree_writable};
use crate::skill_store_operations::combine_operation_errors;
use crate::skill_store_secure_roots::{
    PreparedStoreDirectory, ensure_directory as ensure_prepared_directory, opened_tree_snapshot,
    remove_opened_tree, reserve_opened_directory as reserve_opened_prepared_directory,
};
use serde_json::json;
use std::path::Path;

impl SkillRevisionStore {
    pub(crate) async fn isolate_promotion_residue(
        &self,
        revision_id: &str,
        destination: &PreparedStoreDirectory,
    ) -> anyhow::Result<std::path::PathBuf> {
        ensure_prepared_directory(self.paths.quarantine_identity(), Path::new(".maintenance"))
            .await?;
        let name = format!("{revision_id}-{}", uuid::Uuid::new_v4());
        let relative = Path::new(".maintenance").join(&name);
        let isolated = self.paths.quarantine.join(&relative);
        let isolated_directory =
            reserve_opened_prepared_directory(self.paths.quarantine_identity(), &relative).await?;
        if let Err(error) = copy_prepared_package_tree_into_prepared(
            destination,
            &isolated_directory,
            self.limits.package_limits(),
            &self.faults,
            StoreFaultPoint::QuarantineCopyFile,
        )
        .await
        {
            let cleanup = remove_opened_tree(&isolated_directory).await;
            return Err(combine_operation_errors(
                error,
                [("maintenance isolation cleanup", cleanup)],
            ));
        }
        destination.verify()?;
        make_tree_writable(destination, self.limits.package_limits()).await?;
        remove_opened_tree(destination).await?;
        Ok(isolated)
    }

    pub(crate) async fn actual_quarantine_metadata(
        &self,
        record: &SkillRevisionRecord,
        source: &PreparedStoreDirectory,
        reason: &str,
    ) -> anyhow::Result<SkillRevisionMetadata> {
        let tree = opened_tree_snapshot(source, self.limits.package_limits()).await?;
        let mut validation = json!({
            "status": "invalid",
            "validationError": reason,
        });
        let (version, descriptor_json) = match tree.load_descriptor(source.path()) {
            Ok(loaded) => (
                loaded.descriptor.version.to_string(),
                serde_json::to_value(&loaded.descriptor)?,
            ),
            Err(error) => {
                validation["descriptorError"] = json!(format!("{error:#}"));
                (record.version.clone(), record.descriptor_json.clone())
            }
        };
        Ok(SkillRevisionMetadata {
            version,
            content_hash: tree.content_hash,
            descriptor_json,
            validation_json: validation,
        })
    }
}
