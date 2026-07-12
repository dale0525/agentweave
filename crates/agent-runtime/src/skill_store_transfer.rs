use crate::skill_package::{LoadedPackageDescriptor, SkillPackageId, SkillPackageKind};
use crate::skill_state::{NewSkillRevision, SkillRevisionRecord, SkillRevisionStatus};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::copy_prepared_package_tree_into_prepared;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_operations::{error_is_not_found, storage_path, with_compensation};
use crate::skill_store_public_types::SkillStoreBoundaryError;
use crate::skill_store_secure_roots::{
    open_prepared_directory, opened_package_snapshot, remove_opened_tree, reserve_opened_directory,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::Mutex as AsyncMutex;

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
        let directory = open_prepared_directory(root, relative)
            .await
            .map_err(|error| {
                if error_is_not_found(&error) {
                    SkillStoreBoundaryError::NotFound(error)
                } else {
                    SkillStoreBoundaryError::InvalidInput(error)
                }
            })?;
        let snapshot = opened_package_snapshot(&directory, self.import_limits())
            .await
            .map_err(SkillStoreBoundaryError::InvalidInput)?;
        directory
            .verify()
            .map_err(SkillStoreBoundaryError::Conflict)?;
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
        package_id: &SkillPackageId,
        expected_hash: &str,
        actor_id: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        root.verify("skill import")?;
        self.paths.verify_identity()?;
        let source = open_prepared_directory(root, relative).await?;
        let source_snapshot = opened_package_snapshot(&source, self.import_limits()).await?;
        if source_snapshot.content_hash != expected_hash
            || source_snapshot.descriptor.descriptor.id != *package_id
        {
            anyhow::bail!("import package changed after inspection");
        }
        let key = format!(
            "{}\0{}\0{}",
            self.paths.quarantine.display(),
            package_id.as_str(),
            expected_hash
        );
        let _import_guard = import_lock(&key).lock_owned().await;
        if let Some(existing) = self
            .find_inactive_import_replay(package_id, expected_hash)
            .await?
        {
            return self.verified_inactive_replay(existing).await;
        }
        let revision_id = crate::skill_state::SkillStateStore::allocate_revision_id();
        let destination = self.paths.quarantine.join(&revision_id);
        let reserved =
            reserve_opened_directory(self.paths.quarantine_identity(), Path::new(&revision_id))
                .await?;
        self.faults
            .checkpoint(StoreFaultPoint::ImportAfterReserve)
            .await;
        let result = async {
            copy_prepared_package_tree_into_prepared(
                &source,
                &reserved,
                self.import_limits(),
                &self.faults,
                StoreFaultPoint::IncomingCopyFile,
            )
            .await?;
            self.faults
                .checkpoint(StoreFaultPoint::ImportAfterCopy)
                .await;
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
            self.faults
                .checkpoint(StoreFaultPoint::ImportBeforeRow)
                .await;
            self.faults.check(StoreFaultPoint::ImportBeforeRow)?;
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
            self.faults
                .checkpoint(StoreFaultPoint::ImportAfterRow)
                .await;
            self.faults
                .checkpoint(StoreFaultPoint::ImportBeforeFinalize)
                .await;
            Ok(record)
        }
        .await;
        let result = match result {
            Ok(revision) => Ok(revision),
            Err(error) => {
                let cleanup = remove_opened_tree(&reserved).await;
                let injected_cleanup = self.faults.check(StoreFaultPoint::TransferCleanup);
                match (cleanup, injected_cleanup) {
                    (Ok(()), Ok(())) => Err(error),
                    (Err(cleanup), Ok(())) => Err(with_compensation(error, cleanup)),
                    (Ok(()), Err(cleanup)) | (Err(_), Err(cleanup)) => {
                        Err(with_compensation(error, cleanup))
                    }
                }
            }
        };
        self.faults
            .checkpoint(StoreFaultPoint::ImportTerminal)
            .await;
        result
    }

    pub(crate) async fn export_managed_revision(
        &self,
        record: &SkillRevisionRecord,
        root: &StoreRootIdentity,
        relative: &Path,
    ) -> anyhow::Result<PathBuf> {
        if record.status != SkillRevisionStatus::Managed {
            return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                "only managed revisions can be exported"
            ))
            .into());
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
        let destination = match reserve_opened_directory(root, relative).await {
            Ok(destination) => destination,
            Err(reserve_error) => match open_prepared_directory(root, relative).await {
                Ok(existing) => {
                    let existing_snapshot =
                        opened_package_snapshot(&existing, self.limits.package_limits()).await?;
                    if existing_snapshot.content_hash == record.content_hash {
                        return Ok(root.path().join(relative));
                    }
                    return Err(SkillStoreBoundaryError::Conflict(anyhow::anyhow!(
                        "export destination conflicts with different content"
                    ))
                    .into());
                }
                Err(_) => return Err(reserve_error),
            },
        };
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

    async fn find_inactive_import_replay(
        &self,
        package_id: &SkillPackageId,
        content_hash: &str,
    ) -> anyhow::Result<Option<SkillRevisionRecord>> {
        let query = format!(
            "SELECT {} FROM skill_revisions WHERE package_id = ? AND content_hash = ? AND lifecycle_status IN ('staging', 'quarantined') ORDER BY created_at, revision_id LIMIT 1",
            crate::skill_state_rows::REVISION_COLUMNS
        );
        sqlx::query(&query)
            .bind(package_id.as_str())
            .bind(content_hash)
            .fetch_optional(self.state.pool())
            .await?
            .map(|row| crate::skill_state_rows::revision_from_row(&row))
            .transpose()
    }

    async fn verified_inactive_replay(
        &self,
        record: SkillRevisionRecord,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let (identity, relative, path) = match record.status {
            SkillRevisionStatus::Staging => {
                let (path, relative) = self.staging_revision_path(&record)?;
                (self.paths.staging_identity(), relative, path)
            }
            SkillRevisionStatus::Quarantined => (
                self.paths.quarantine_identity(),
                PathBuf::from(&record.revision_id),
                self.paths.quarantine.join(&record.revision_id),
            ),
            SkillRevisionStatus::Managed => {
                anyhow::bail!("managed revision is not an inactive import replay")
            }
        };
        let directory = open_prepared_directory(identity, &relative).await?;
        let snapshot = opened_package_snapshot(&directory, self.import_limits()).await?;
        if snapshot.content_hash != record.content_hash
            || snapshot.descriptor.descriptor.id != record.package_id
        {
            anyhow::bail!("inactive import replay bytes do not match recorded metadata");
        }
        let _ = path;
        Ok(record)
    }
}

fn import_locks() -> &'static Mutex<std::collections::HashMap<String, Weak<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<Mutex<std::collections::HashMap<String, Weak<AsyncMutex<()>>>>> =
        OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn import_lock(key: &str) -> Arc<AsyncMutex<()>> {
    let mut locks = import_locks()
        .lock()
        .expect("import lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(key.to_string(), Arc::downgrade(&lock));
    lock
}
