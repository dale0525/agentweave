use crate::skill_state::{SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionRecord};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_atomic_write::atomic_replace_file;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::{
    copy_prepared_package_tree_into_prepared, make_tree_writable, remove_regular_file_nofollow,
};
use crate::skill_store_fs_types::StoredFileContents;
use crate::skill_store_operations::{combine_operation_errors, storage_path, with_compensation};
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

    pub(crate) async fn isolate_failed_staging_write(
        &self,
        record: &SkillRevisionRecord,
        source: &Path,
        source_directory: &PreparedStoreDirectory,
        relative_path: &Path,
        previous: Option<&StoredFileContents>,
        primary: anyhow::Error,
    ) -> anyhow::Error {
        let tree = match opened_tree_snapshot(source_directory, self.limits.package_limits()).await
        {
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
        let reserved = reserve_opened_prepared_directory(
            self.paths.quarantine_identity(),
            Path::new(&record.revision_id),
        )
        .await;
        let isolated_directory = match reserved {
            Ok(destination_directory) => {
                let copy = async {
                    self.faults.check(StoreFaultPoint::WriteIsolationCopy)?;
                    copy_prepared_package_tree_into_prepared(
                        source_directory,
                        &destination_directory,
                        self.limits.package_limits(),
                        &self.faults,
                        StoreFaultPoint::QuarantineCopyFile,
                    )
                    .await
                }
                .await;
                match copy {
                    Ok(()) => Some(destination_directory),
                    Err(error) => {
                        let cleanup = remove_opened_tree(&destination_directory).await;
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
                        None
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
                None
            }
        };
        let copied = isolated_directory.is_some();
        let authoritative = if copied { &destination } else { source };
        let database = async {
            self.faults.check(StoreFaultPoint::WriteIsolationDatabase)?;
            self.state
                .quarantine_revision_record_cas(
                    &record.revision_id,
                    &storage_path(authoritative)?,
                    "staging metadata update and file restore both failed",
                    SkillRevisionExpectation::from(record),
                    Some(metadata.clone()),
                )
                .await
        }
        .await;
        match database {
            Ok(_) if copied => {
                let cleanup = remove_opened_tree(source_directory).await;
                combine_operation_errors(primary, [("staging source cleanup", cleanup)])
            }
            Ok(_) => primary,
            Err(error) => {
                let restore = async {
                    self.faults.check(StoreFaultPoint::WriteIsolationRestore)?;
                    match previous {
                        Some(previous) => atomic_replace_file(
                            source_directory,
                            relative_path,
                            &previous.bytes,
                            previous.mode,
                            &self.faults,
                        )
                        .await
                        .map_err(|failure| failure.into_error()),
                        None => remove_regular_file_nofollow(source, relative_path).await,
                    }
                }
                .await;
                match restore {
                    Err(restore_error) if copied => {
                        let retry = async {
                            let destination_path = storage_path(&destination)?;
                            self.state
                                .quarantine_revision_record_cas(
                                    &record.revision_id,
                                    &destination_path,
                                    "staging write recovery could not restore authoritative bytes",
                                    SkillRevisionExpectation::from(record),
                                    Some(metadata),
                                )
                                .await
                        }
                        .await;
                        match retry {
                            Ok(_) => {
                                let cleanup = remove_opened_tree(source_directory).await;
                                combine_operation_errors(
                                    primary,
                                    [
                                        ("write isolation database transition", Err(error)),
                                        ("authoritative staging restore", Err(restore_error)),
                                        ("staging source cleanup", cleanup),
                                    ],
                                )
                            }
                            Err(retry_error) => combine_operation_errors(
                                primary,
                                [
                                    ("write isolation database transition", Err(error)),
                                    ("authoritative staging restore", Err(restore_error)),
                                    ("write isolation database retry", Err(retry_error)),
                                ],
                            ),
                        }
                    }
                    Err(restore_error) => combine_operation_errors(
                        primary,
                        [
                            ("write isolation database transition", Err(error)),
                            ("authoritative staging restore", Err(restore_error)),
                        ],
                    ),
                    Ok(()) => {
                        let cleanup = if copied {
                            remove_opened_tree(
                                isolated_directory.as_ref().expect("copied isolation"),
                            )
                            .await
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
        }
    }
}
