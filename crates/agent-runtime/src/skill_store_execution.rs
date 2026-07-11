use crate::skill_package::SkillPackageId;
use crate::skill_state::{SkillInstallStatus, SkillLayerRecord, SkillRevisionStatus};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::{PackageLimits, copy_prepared_package_tree_into_reserved};
use crate::skill_store_locks::{RevisionOperationGuard, acquire_revision_lock};
use crate::skill_store_operations::ensure_exact_path;
use crate::skill_store_secure_fs::secure_package_hash;
use crate::skill_store_secure_roots::open_prepared_directory;
use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) struct PreparedSkillExecution {
    root: PathBuf,
    _temporary: tempfile::TempDir,
    _guard: RevisionOperationGuard,
}

impl PreparedSkillExecution {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

impl SkillRevisionStore {
    pub(crate) async fn prepare_managed_execution(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        expected_path: &Path,
        expected_hash: &str,
        limits: PackageLimits,
    ) -> anyhow::Result<PreparedSkillExecution> {
        let guard = acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        self.paths.verify_identity()?;
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("managed execution revision not found: {revision_id}"))?;
        let installation = self
            .state
            .get_installation(package_id)
            .await?
            .with_context(|| {
                format!(
                    "managed execution installation not found: {}",
                    package_id.as_str()
                )
            })?;
        if record.status != SkillRevisionStatus::Managed
            || &record.package_id != package_id
            || record.content_hash != expected_hash
            || installation.source_layer != SkillLayerRecord::Managed
            || installation.status != SkillInstallStatus::Active
            || !installation.enabled
            || installation.active_revision_id.as_deref() != Some(revision_id)
        {
            anyhow::bail!("no longer active managed revision: {revision_id}");
        }
        ensure_exact_path(
            Path::new(&record.storage_path),
            expected_path,
            "managed execution",
        )?;
        let relative = PathBuf::from(package_id.as_str())
            .join("revisions")
            .join(revision_id);
        let managed_directory =
            open_prepared_directory(self.paths.managed_identity(), &relative).await?;
        let temporary = tempfile::Builder::new()
            .prefix("general-agent-skill-execution-")
            .tempdir()?;
        copy_prepared_package_tree_into_reserved(
            &managed_directory,
            temporary.path(),
            limits,
            &self.faults,
            StoreFaultPoint::ExecutionCopyFile,
        )
        .await?;
        let actual = secure_package_hash(temporary.path(), limits).await?;
        if actual != expected_hash {
            anyhow::bail!("managed execution snapshot hash mismatch: {revision_id}");
        }
        self.faults
            .checkpoint(StoreFaultPoint::ExecutionAfterSnapshot)
            .await;
        let root = temporary.path().to_path_buf();
        Ok(PreparedSkillExecution {
            root,
            _temporary: temporary,
            _guard: guard,
        })
    }
}
