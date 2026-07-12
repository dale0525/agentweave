use super::*;
use crate::skill_state::SkillStateBoundaryError;
use crate::skill_store_locks::RevisionOperationGuard;
use crate::skill_store_secure_roots::{
    PreparedStoreDirectory, open_prepared_directory, opened_tree_snapshot,
};

pub(crate) struct PreparedInvalidManagedRevision {
    record: SkillRevisionRecord,
    source: PathBuf,
    directory: PreparedStoreDirectory,
    observed_content_hash: String,
    _guard: RevisionOperationGuard,
}

impl SkillRevisionStore {
    pub(crate) async fn prepare_invalid_managed_revision(
        &self,
        expected: &SkillRevisionRecord,
    ) -> anyhow::Result<Option<PreparedInvalidManagedRevision>> {
        if expected.status != SkillRevisionStatus::Managed {
            return Err(recovery_conflict(
                "recovery quarantine requires a managed revision",
            ));
        }
        let source = self.expected_revision_path(expected)?;
        let guard =
            acquire_revision_lock(&self.paths.identity, &expected.revision_id, &self.faults)
                .await?;
        let current = self
            .state
            .get_revision(&expected.revision_id)
            .await?
            .ok_or_else(|| recovery_conflict("recovery revision disappeared"))?;
        if current != *expected {
            return Err(recovery_conflict(
                "recovery revision changed before final verification",
            ));
        }
        let relative = PathBuf::from(expected.package_id.as_str())
            .join("revisions")
            .join(&expected.revision_id);
        let directory = open_prepared_directory(self.paths.managed_identity(), &relative).await?;
        let tree = opened_tree_snapshot(&directory, self.limits.package_limits()).await?;
        directory.verify()?;
        if recovery_tree_matches_record(&tree, &directory, expected)? {
            return Ok(None);
        }
        Ok(Some(PreparedInvalidManagedRevision {
            record: current,
            source,
            directory,
            observed_content_hash: tree.content_hash,
            _guard: guard,
        }))
    }

    pub(crate) async fn quarantine_prepared_invalid_managed_revision(
        &self,
        prepared: PreparedInvalidManagedRevision,
        reason: &str,
    ) -> anyhow::Result<StoredSkillRevision> {
        prepared.directory.verify()?;
        let current = self
            .state
            .get_revision(&prepared.record.revision_id)
            .await?
            .ok_or_else(|| recovery_conflict("recovery revision disappeared"))?;
        if current != prepared.record {
            return Err(recovery_conflict(
                "recovery revision changed before quarantine",
            ));
        }
        let tree = opened_tree_snapshot(&prepared.directory, self.limits.package_limits()).await?;
        prepared.directory.verify()?;
        if tree.content_hash != prepared.observed_content_hash
            || recovery_tree_matches_record(&tree, &prepared.directory, &prepared.record)?
        {
            return Err(recovery_conflict("recovery tree changed before quarantine"));
        }
        self.quarantine_opened_revision(
            prepared.record,
            prepared.source,
            prepared.directory,
            reason,
        )
        .await
    }
}

fn recovery_tree_matches_record(
    tree: &crate::skill_store_secure_snapshot::SecureTreeSnapshot,
    directory: &PreparedStoreDirectory,
    record: &SkillRevisionRecord,
) -> anyhow::Result<bool> {
    if tree.content_hash != record.content_hash {
        return Ok(false);
    }
    let Ok(loaded) = tree.load_descriptor(directory.path()) else {
        return Ok(false);
    };
    Ok(loaded.descriptor.id == record.package_id
        && loaded.descriptor.version.to_string() == record.version
        && serde_json::to_value(&loaded.descriptor)? == record.descriptor_json)
}

fn recovery_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}
