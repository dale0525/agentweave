use crate::skill_store::{SkillRevisionStore, SkillStoreMaintenanceIssue};
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::make_tree_writable;
use crate::skill_store_operations::with_compensation;
use crate::skill_store_secure_roots::{PreparedStoreDirectory, remove_opened_tree};
use chrono::Utc;
use std::path::Path;

impl SkillRevisionStore {
    pub(crate) async fn cleanup_staging_candidate_error(
        &self,
        revision_id: &str,
        primary: anyhow::Error,
        candidate: &PreparedStoreDirectory,
    ) -> anyhow::Result<()> {
        let writable = make_tree_writable(candidate, self.limits.package_limits()).await;
        let cleanup = match self.faults.check(StoreFaultPoint::WriteCandidateCleanup) {
            Ok(()) => remove_opened_tree(candidate).await,
            Err(error) => Err(error),
        };
        let mut diagnostics = Vec::new();
        for (operation, result) in [
            ("staging_candidate_writable", writable),
            ("staging_candidate_cleanup", cleanup),
        ] {
            if let Err(error) = result {
                self.record_maintenance_issue(revision_id, operation, candidate.path(), &error);
                diagnostics.push(format!("{operation}: {error:#}"));
            }
        }
        if diagnostics.is_empty() {
            Err(primary)
        } else {
            let primary_message = format!("{primary:#}");
            Err(primary.context(format!(
                "{primary_message}; candidate cleanup diagnostics: {}",
                diagnostics.join("; ")
            )))
        }
    }

    pub(crate) async fn cleanup_failed_promotion_destination(
        &self,
        destination: &PreparedStoreDirectory,
    ) -> anyhow::Result<()> {
        self.faults.check(StoreFaultPoint::PromoteRestoreRename)?;
        self.faults
            .check(StoreFaultPoint::PromoteDestinationCleanup)?;
        destination.verify()?;
        make_tree_writable(destination, self.limits.package_limits()).await?;
        remove_opened_tree(destination).await?;
        self.faults
            .check(StoreFaultPoint::PromoteDestinationCleanupAfter)
    }

    pub(crate) async fn cleanup_incoming_error<T>(
        &self,
        error: anyhow::Error,
        incoming: &PreparedStoreDirectory,
    ) -> anyhow::Result<T> {
        let cleanup = async {
            incoming.verify()?;
            match tokio::fs::symlink_metadata(incoming.path()).await {
                Ok(_) => make_tree_writable(incoming, self.limits.package_limits()).await?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            remove_opened_tree(incoming).await
        }
        .await;
        match cleanup {
            Ok(()) => Err(error),
            Err(compensation) => Err(with_compensation(error, compensation)),
        }
    }

    pub(crate) async fn cleanup_failed_quarantine_destination(
        &self,
        destination: &PreparedStoreDirectory,
    ) -> anyhow::Result<()> {
        self.faults
            .check(StoreFaultPoint::QuarantineRestoreRename)?;
        self.faults
            .check(StoreFaultPoint::QuarantineDestinationCleanup)?;
        destination.verify()?;
        make_tree_writable(destination, self.limits.package_limits()).await?;
        remove_opened_tree(destination).await?;
        self.faults
            .check(StoreFaultPoint::QuarantineDestinationCleanupAfter)
    }

    pub(crate) async fn cleanup_promoted_source(
        &self,
        source: &PreparedStoreDirectory,
    ) -> anyhow::Result<()> {
        self.faults
            .checkpoint(StoreFaultPoint::PromoteSourceCleanupBeforeApply)
            .await;
        self.faults.check(StoreFaultPoint::PromoteSourceCleanup)?;
        source.verify()?;
        make_tree_writable(source, self.limits.package_limits()).await?;
        remove_opened_tree(source).await?;
        self.faults
            .check(StoreFaultPoint::PromoteSourceCleanupAfter)
    }

    pub(crate) async fn cleanup_quarantined_source(
        &self,
        source: &PreparedStoreDirectory,
    ) -> anyhow::Result<()> {
        self.faults
            .check(StoreFaultPoint::QuarantineSourceCleanup)?;
        source.verify()?;
        make_tree_writable(source, self.limits.package_limits()).await?;
        remove_opened_tree(source).await?;
        self.faults
            .check(StoreFaultPoint::QuarantineSourceCleanupAfter)
    }

    pub(crate) fn record_maintenance_issue(
        &self,
        revision_id: &str,
        operation: &str,
        path: &Path,
        error: &anyhow::Error,
    ) -> SkillStoreMaintenanceIssue {
        let issue = SkillStoreMaintenanceIssue {
            revision_id: revision_id.to_string(),
            operation: operation.to_string(),
            path: path.to_path_buf(),
            message: format!("{error:#}"),
            recorded_at: Utc::now(),
        };
        self.maintenance_issues
            .write()
            .expect("skill store maintenance issue lock poisoned")
            .push(issue.clone());
        issue
    }
}
