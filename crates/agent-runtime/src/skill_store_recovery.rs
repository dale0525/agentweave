use crate::skill_state::{SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionRecord};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::{
    atomic_replace_file, copy_package_tree_into_reserved, remove_regular_file_nofollow,
};
use crate::skill_store_fs_types::StoredFileContents;
use crate::skill_store_operations::{combine_operation_errors, storage_path, with_compensation};
use crate::skill_store_secure_fs::{reserve_store_directory, secure_tree_snapshot};
use serde_json::json;
use std::path::Path;

impl SkillRevisionStore {
    pub(crate) async fn actual_quarantine_metadata(
        &self,
        record: &SkillRevisionRecord,
        source: &Path,
        reason: &str,
    ) -> anyhow::Result<SkillRevisionMetadata> {
        let tree = secure_tree_snapshot(source, self.limits.package_limits()).await?;
        let mut validation = json!({
            "status": "invalid",
            "validationError": reason,
        });
        let (version, descriptor_json) = match tree.load_descriptor(source) {
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

    pub(crate) async fn isolate_failed_staging_write(
        &self,
        record: &SkillRevisionRecord,
        source: &Path,
        relative_path: &Path,
        previous: Option<&StoredFileContents>,
        primary: anyhow::Error,
    ) -> anyhow::Error {
        let tree = match secure_tree_snapshot(source, self.limits.package_limits()).await {
            Ok(tree) => tree,
            Err(error) => return with_compensation(primary, error),
        };
        let metadata = match tree.load_descriptor(source).and_then(|loaded| {
            loaded.descriptor.validate()?;
            if loaded.descriptor.id != record.package_id {
                anyhow::bail!("recovery descriptor package does not match revision record");
            }
            Ok(SkillRevisionMetadata {
                version: loaded.descriptor.version.to_string(),
                content_hash: tree.content_hash.clone(),
                descriptor_json: serde_json::to_value(&loaded.descriptor)?,
                validation_json: json!({"status": "valid"}),
            })
        }) {
            Ok(metadata) => metadata,
            Err(error) => SkillRevisionMetadata {
                version: record.version.clone(),
                content_hash: tree.content_hash,
                descriptor_json: record.descriptor_json.clone(),
                validation_json: json!({
                    "status": "invalid",
                    "descriptorError": format!("{error:#}"),
                }),
            },
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
                        let cleanup = self.remove_store_tree_path(&destination).await;
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
                let cleanup = self.remove_store_tree_path(source).await;
                combine_operation_errors(primary, [("staging source cleanup", cleanup)])
            }
            Ok(_) => primary,
            Err(error) => {
                let restore = match previous {
                    Some(previous) => atomic_replace_file(
                        source,
                        relative_path,
                        &previous.bytes,
                        previous.mode,
                        &self.faults,
                    )
                    .await
                    .map_err(|failure| failure.into_error()),
                    None => remove_regular_file_nofollow(source, relative_path).await,
                };
                let cleanup = if copied && restore.is_ok() {
                    self.remove_store_tree_path(&destination).await
                } else {
                    Ok(())
                };
                combine_operation_errors(
                    primary,
                    [
                        ("write isolation database transition", Err(error)),
                        ("authoritative staging restore", restore),
                        ("write isolation copy cleanup", cleanup),
                    ],
                )
            }
        }
    }
}
