use crate::skill_package::{LoadedPackageDescriptor, SkillPackageKind};
use crate::skill_state::{NewSkillRevision, SkillRevisionRecord, SkillRevisionStatus};
use crate::skill_store::{SkillRevisionStore, StoredSkillRevision};
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::copy_prepared_package_tree_into_prepared;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_operations::{storage_path, stored_revision, with_compensation};
use crate::skill_store_secure_roots::{
    open_prepared_directory, opened_package_snapshot, remove_opened_tree, reserve_opened_directory,
};
use serde_json::json;
use std::path::{Path, PathBuf};

const IMPORT_MAX_FILE_BYTES: u64 = 256 * 1024;

pub(crate) struct TransferPackageSnapshot {
    pub descriptor: LoadedPackageDescriptor,
    pub content_hash: String,
    pub has_runtime_manifest: bool,
}

impl SkillRevisionStore {
    pub(crate) async fn release_quarantined_revision(
        &self,
        record: SkillRevisionRecord,
        validation_json: serde_json::Value,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let store = self.clone();
        tokio::spawn(async move {
            store
                .release_quarantined_revision_inner(record, validation_json)
                .await
        })
        .await
        .map_err(|error| anyhow::anyhow!("quarantine release task failed: {error}"))?
    }

    async fn release_quarantined_revision_inner(
        &self,
        record: SkillRevisionRecord,
        validation_json: serde_json::Value,
    ) -> anyhow::Result<SkillRevisionRecord> {
        if record.status != SkillRevisionStatus::Quarantined {
            anyhow::bail!("only quarantined revisions can be released to staging");
        }
        let _guard = crate::skill_store_locks::acquire_revision_lock(
            &self.paths.identity,
            &record.revision_id,
            &self.faults,
        )
        .await?;
        let current = self
            .state
            .get_revision(&record.revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill revision not found"))?;
        if current != record {
            anyhow::bail!("skill revision changed before quarantine release");
        }
        let source = open_prepared_directory(
            self.paths.quarantine_identity(),
            Path::new(&record.revision_id),
        )
        .await?;
        let destination = reserve_opened_directory(
            self.paths.staging_identity(),
            Path::new(&record.revision_id),
        )
        .await?;
        let result = async {
            copy_prepared_package_tree_into_prepared(
                &source,
                &destination,
                self.import_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await?;
            let snapshot = opened_package_snapshot(&destination, self.import_limits()).await?;
            if snapshot.content_hash != record.content_hash {
                anyhow::bail!("quarantine release hash mismatch");
            }
            self.state
                .release_quarantined_revision_cas(
                    &record.revision_id,
                    crate::skill_state::SkillRevisionExpectation::from(&record),
                    &storage_path(destination.path())?,
                    validation_json,
                )
                .await
        }
        .await;
        let released = match result {
            Ok(released) => released,
            Err(error) => {
                return match remove_opened_tree(&destination).await {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(with_compensation(error, cleanup)),
                };
            }
        };
        if let Err(error) = remove_opened_tree(&source).await {
            self.record_maintenance_issue(
                &record.revision_id,
                "quarantine_release_source_cleanup",
                source.path(),
                &error,
            );
        }
        Ok(released)
    }

    pub(crate) async fn inspect_transfer_package(
        &self,
        root: &StoreRootIdentity,
        relative: &Path,
    ) -> anyhow::Result<TransferPackageSnapshot> {
        root.verify("skill import")?;
        let directory = open_prepared_directory(root, relative).await?;
        let snapshot = opened_package_snapshot(&directory, self.import_limits()).await?;
        directory.verify()?;
        Ok(TransferPackageSnapshot {
            descriptor: snapshot.descriptor,
            content_hash: snapshot.content_hash,
            has_runtime_manifest: snapshot.runtime_manifest.is_some(),
        })
    }

    pub(crate) async fn import_quarantined_revision(
        &self,
        root: &StoreRootIdentity,
        relative: &Path,
        expected_hash: &str,
        actor_id: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        root.verify("skill import")?;
        self.paths.verify_identity()?;
        let source = open_prepared_directory(root, relative).await?;
        let revision_id = crate::skill_state::SkillStateStore::allocate_revision_id();
        let destination = self.paths.quarantine.join(&revision_id);
        let reserved =
            reserve_opened_directory(self.paths.quarantine_identity(), Path::new(&revision_id))
                .await?;
        let result = async {
            copy_prepared_package_tree_into_prepared(
                &source,
                &reserved,
                self.import_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await?;
            let snapshot = opened_package_snapshot(&reserved, self.import_limits()).await?;
            if snapshot.content_hash != expected_hash {
                anyhow::bail!("import package changed after inspection");
            }
            if snapshot.descriptor.descriptor.kind == SkillPackageKind::NativeRuntime
                || snapshot.runtime_manifest.is_some()
            {
                anyhow::bail!("native runtime imports are disabled by default");
            }
            let descriptor = snapshot.descriptor.descriptor;
            let record = self
                .state
                .create_quarantined_revision_record(
                    &revision_id,
                    NewSkillRevision {
                        package_id: descriptor.id.clone(),
                        version: descriptor.version.to_string(),
                        content_hash: snapshot.content_hash,
                        storage_path: storage_path(&destination)?,
                        descriptor_json: serde_json::to_value(descriptor)?,
                        validation_json: json!({"ok": false, "status": "quarantined"}),
                        created_by: actor_id.to_string(),
                    },
                )
                .await?;
            Ok(stored_revision(record, destination.clone(), Vec::new()))
        }
        .await;
        match result {
            Ok(revision) => Ok(revision),
            Err(error) => match remove_opened_tree(&reserved).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(with_compensation(error, cleanup)),
            },
        }
    }

    pub(crate) async fn export_managed_revision(
        &self,
        record: &SkillRevisionRecord,
        root: &StoreRootIdentity,
        relative: &Path,
    ) -> anyhow::Result<PathBuf> {
        if record.status != SkillRevisionStatus::Managed {
            anyhow::bail!("only managed revisions can be exported");
        }
        root.verify("skill export")?;
        self.paths.verify_identity()?;
        self.expected_revision_path(record)?;
        let source_relative = PathBuf::from(record.package_id.as_str())
            .join("revisions")
            .join(&record.revision_id);
        let source =
            open_prepared_directory(self.paths.managed_identity(), &source_relative).await?;
        let snapshot = opened_package_snapshot(&source, self.limits.package_limits()).await?;
        if snapshot.content_hash != record.content_hash {
            anyhow::bail!("managed revision bytes do not match recorded content hash");
        }
        let destination = reserve_opened_directory(root, relative).await?;
        let result = async {
            copy_prepared_package_tree_into_prepared(
                &source,
                &destination,
                self.limits.package_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await?;
            let copied =
                opened_package_snapshot(&destination, self.limits.package_limits()).await?;
            if copied.content_hash != record.content_hash {
                anyhow::bail!("exported package hash mismatch");
            }
            Ok(root.path().join(relative))
        }
        .await;
        match result {
            Ok(path) => Ok(path),
            Err(error) => match remove_opened_tree(&destination).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(with_compensation(error, cleanup)),
            },
        }
    }

    fn import_limits(&self) -> crate::skill_store_fs::PackageLimits {
        let mut limits = self.limits.package_limits();
        limits.max_file_bytes = limits.max_file_bytes.min(IMPORT_MAX_FILE_BYTES);
        limits
    }
}
