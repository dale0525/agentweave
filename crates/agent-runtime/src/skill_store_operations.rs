use crate::skill_state::SkillRevisionRecord;
use crate::skill_store::{SkillStoreMaintenanceIssue, StoredSkillRevision};
use crate::skill_store_fs::remove_created_directories;
use anyhow::Context;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionPhase {
    Initial,
    IncomingCopied,
    DestinationReserved,
    PermissionsApplied,
    DatabaseCommitted,
    SourceCleanupAttempted,
}

pub(crate) struct TransitionState {
    operation: &'static str,
    phase: TransitionPhase,
}

impl TransitionState {
    pub(crate) fn new(operation: &'static str) -> Self {
        Self {
            operation,
            phase: TransitionPhase::Initial,
        }
    }

    pub(crate) fn advance(&mut self, phase: TransitionPhase) {
        self.phase = phase;
    }

    pub(crate) fn context(&self, error: anyhow::Error) -> anyhow::Error {
        anyhow::anyhow!(
            "{} transition failed in {:?} phase: {error:#}",
            self.operation,
            self.phase
        )
    }
}

pub(crate) fn stored_revision(
    record: SkillRevisionRecord,
    path: PathBuf,
    maintenance_issues: Vec<SkillStoreMaintenanceIssue>,
) -> StoredSkillRevision {
    StoredSkillRevision {
        revision_id: record.revision_id,
        package_id: record.package_id,
        path,
        content_hash: record.content_hash,
        maintenance_issues,
    }
}

pub(crate) fn ensure_exact_path(actual: &Path, expected: &Path, label: &str) -> anyhow::Result<()> {
    if actual != expected {
        anyhow::bail!(
            "{label} storage path mismatch: expected {}, found {}",
            expected.display(),
            actual.display()
        );
    }
    Ok(())
}

pub(crate) fn storage_path(path: &Path) -> anyhow::Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .with_context(|| format!("skill storage path must be UTF-8: {}", path.display()))
}

pub(crate) fn combine_operation_errors<const N: usize>(
    primary: anyhow::Error,
    compensations: [(&str, anyhow::Result<()>); N],
) -> anyhow::Error {
    let failures = compensations
        .into_iter()
        .filter_map(|(label, result)| result.err().map(|error| format!("{label}: {error:#}")))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        primary
    } else {
        primary.context(format!("compensation failed: {}", failures.join("; ")))
    }
}

pub(crate) fn with_compensation(
    primary: anyhow::Error,
    compensation: anyhow::Error,
) -> anyhow::Error {
    primary.context(format!("compensation failed: {compensation:#}"))
}

pub(crate) async fn cleanup_created_directories_error(
    error: anyhow::Error,
    created_directories: &[PathBuf],
) -> anyhow::Result<()> {
    match remove_created_directories(created_directories).await {
        Ok(()) => Err(error),
        Err(cleanup) => Err(with_compensation(error, cleanup)),
    }
}
