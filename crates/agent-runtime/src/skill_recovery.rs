use crate::skill_snapshot::SkillSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    CurrentSnapshotValid,
    NewSnapshotPublished,
    LastKnownGoodRestored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRecoveryReport {
    pub status: RecoveryStatus,
    pub generation: u64,
    pub quarantined_revisions: Vec<String>,
    pub maintenance_diagnostics: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillCleanupReport {
    pub deleted_revisions: Vec<String>,
    pub retained_revisions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedSnapshotMember {
    pub package_id: String,
    pub version: String,
    pub content_hash: String,
    pub layer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
}

pub(crate) fn snapshot_members(snapshot: &SkillSnapshot) -> serde_json::Value {
    let mut members = snapshot
        .packages()
        .iter()
        .map(|resolved| {
            let revision_id = resolved
                .package
                .verified_content
                .as_ref()
                .and_then(|content| content.execution_binding.as_ref())
                .map(|binding| binding.revision_id.clone());
            PersistedSnapshotMember {
                package_id: resolved.package.descriptor.id.as_str().to_string(),
                version: resolved.package.descriptor.version.to_string(),
                content_hash: resolved.package.content_hash.clone(),
                layer: match resolved.package.layer {
                    crate::skill_source::SkillLayer::Builtin => "builtin",
                    crate::skill_source::SkillLayer::Managed => "managed",
                    crate::skill_source::SkillLayer::Session => "session",
                }
                .into(),
                revision_id,
            }
        })
        .collect::<Vec<_>>();
    members.sort_by(|left, right| {
        left.package_id
            .cmp(&right.package_id)
            .then_with(|| left.layer.cmp(&right.layer))
    });
    serde_json::to_value(members).expect("snapshot members must serialize")
}

pub(crate) fn parse_snapshot_members(
    value: serde_json::Value,
) -> anyhow::Result<Vec<PersistedSnapshotMember>> {
    Ok(serde_json::from_value(value)?)
}

#[async_trait::async_trait]
impl crate::tools::ToolExecutionObserver for crate::skill_manager::SkillManager {
    async fn finished(
        &self,
        source: &crate::tools::ToolSource,
        success: bool,
    ) -> anyhow::Result<()> {
        self.record_execution_result(source, success).await
    }
}

impl crate::skill_manager::SkillManager {
    pub async fn cleanup_unreferenced_revisions(&self) -> anyhow::Result<SkillCleanupReport> {
        let _publication = self.begin_publication().await?;
        let backend = self.managed_runtime()?;
        let current = self.current_snapshot();
        let (mut live_generations, mut protected) = self.live_snapshot_protections();
        let (durable_generations, durable_revisions) =
            backend.state.durable_snapshot_protections().await?;
        live_generations.extend(durable_generations);
        protected.extend(durable_revisions);
        protected.extend(snapshot_revision_ids(&current));
        protected.extend(backend.state.lifecycle_protected_revision_ids().await?);

        let snapshots = backend.state.list_snapshot_records().await?;
        let active_generation = snapshots
            .iter()
            .find(|snapshot| snapshot.status == crate::skill_state::SkillSnapshotStatus::Active)
            .map_or(current.generation(), |snapshot| snapshot.generation);
        for snapshot in &snapshots {
            let historical = snapshot.status == crate::skill_state::SkillSnapshotStatus::Candidate
                && snapshot.generation < active_generation
                && !live_generations.contains(&snapshot.generation);
            if historical {
                backend
                    .state
                    .delete_historical_snapshot_candidate_cas(snapshot)
                    .await?;
            }
        }
        for snapshot in backend.state.list_snapshot_records().await? {
            for member in parse_snapshot_members(snapshot.members_json)? {
                if let Some(revision_id) = member.revision_id {
                    protected.insert(revision_id);
                }
            }
        }

        let mut deleted = Vec::new();
        let mut retained = Vec::new();
        for record in backend.state.list_managed_revisions().await? {
            if protected.contains(&record.revision_id) {
                retained.push(record.revision_id);
                continue;
            }
            let _revision_guard = backend
                .revisions
                .acquire_revision_operation_lock(&record.revision_id)
                .await?;
            backend
                .revisions
                .checkpoint(crate::skill_store_faults::StoreFaultPoint::CleanupBeforePrepare)
                .await;
            let (_, process_revisions) = self.live_snapshot_protections();
            if process_revisions.contains(&record.revision_id) {
                retained.push(record.revision_id);
                continue;
            }
            if !backend.state.prepare_revision_cleanup(&record).await? {
                retained.push(record.revision_id);
                continue;
            }
            if let Err(error) = backend
                .revisions
                .delete_managed_revision_tree(&record, &_revision_guard)
                .await
            {
                return Err(
                    match record_cleanup_failure(&backend, &record, "tree_delete").await {
                        Ok(()) => error,
                        Err(record_error) => error.context(format!(
                            "failed to record retryable cleanup outcome: {record_error:#}"
                        )),
                    },
                );
            }
            if let Err(error) = backend.state.finish_revision_cleanup(&record).await {
                return Err(
                    match record_cleanup_failure(&backend, &record, "state_finalize").await {
                        Ok(()) => error,
                        Err(record_error) => error.context(format!(
                            "failed to record retryable cleanup outcome: {record_error:#}"
                        )),
                    },
                );
            }
            deleted.push(record.revision_id);
        }
        deleted.sort();
        retained.sort();
        Ok(SkillCleanupReport {
            deleted_revisions: deleted,
            retained_revisions: retained,
        })
    }
}

async fn record_cleanup_failure(
    backend: &crate::skill_manager::ManagedRuntimeBackend,
    record: &crate::skill_state::SkillRevisionRecord,
    phase: &str,
) -> anyhow::Result<()> {
    let key = format!("cleanup:{}:{phase}", record.revision_id);
    backend
        .state
        .record_maintenance_diagnostic_once(
            &key,
            Some(&record.revision_id),
            "managed",
            "cleanup_unreferenced_revision_failed",
            serde_json::json!({"phase": phase, "outcome": "retryable"}),
        )
        .await?;
    backend
        .state
        .record_cleanup_failure_audit(record, phase)
        .await?;
    Ok(())
}

pub(crate) fn snapshot_revision_ids(snapshot: &SkillSnapshot) -> BTreeSet<String> {
    snapshot
        .packages()
        .iter()
        .chain(snapshot.inactive())
        .filter_map(|resolved| {
            resolved
                .package
                .verified_content
                .as_ref()
                .and_then(|content| content.execution_binding.as_ref())
                .map(|binding| binding.revision_id.clone())
        })
        .collect()
}

pub(crate) async fn reconcile_startup_residue(
    backend: &crate::skill_manager::ManagedRuntimeBackend,
    fallback_generation: u64,
) -> anyhow::Result<usize> {
    let generation = backend
        .state
        .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
        .await?
        .map_or(fallback_generation, |snapshot| snapshot.generation);
    backend
        .state
        .reconcile_stale_pending_approvals(generation)
        .await?;

    let rows = backend.state.list_all_revisions().await?;
    let enumeration = backend.revisions.enumerate_recovery_trees().await?;
    if enumeration.limit_exceeded {
        backend
            .state
            .record_maintenance_diagnostic_once(
                "startup-enumeration-limit",
                None,
                "store",
                "startup_enumeration_limit_exceeded",
                serde_json::json!({"outcome": "evidence_preserved"}),
            )
            .await?;
    }
    for entry in enumeration.unknown {
        let key = format!(
            "startup-unknown:{}:{}:{}",
            entry.area, entry.kind, entry.name
        );
        backend
            .state
            .record_maintenance_diagnostic_once(
                &key,
                None,
                entry.area,
                "unknown_startup_entry",
                serde_json::json!({
                    "identity": entry.name,
                    "entry_type": entry.kind,
                    "ownership": "unproven"
                }),
            )
            .await?;
    }
    if enumeration.limit_exceeded {
        return backend.state.maintenance_diagnostic_count().await;
    }
    for tree in enumeration.trees {
        let bound = rows
            .iter()
            .find(|record| std::path::Path::new(&record.storage_path) == tree.directory.path());
        match tree.area {
            crate::skill_store_startup::RecoveryTreeArea::Staging => {
                if bound.is_none() {
                    record_tree_diagnostic(
                        backend,
                        "staging",
                        "tree_only_staging",
                        &tree.name,
                        None,
                    )
                    .await?;
                }
            }
            crate::skill_store_startup::RecoveryTreeArea::Quarantine => {
                if bound.is_none() {
                    record_tree_diagnostic(
                        backend,
                        "quarantine",
                        "tree_only_quarantine",
                        &tree.name,
                        None,
                    )
                    .await?;
                }
            }
            crate::skill_store_startup::RecoveryTreeArea::Managed => {
                let row = rows.iter().find(|record| record.revision_id == tree.name);
                match row {
                    Some(record)
                        if record.status == crate::skill_state::SkillRevisionStatus::Staging
                            && tree.package_id.as_ref() == Some(&record.package_id) =>
                    {
                        backend
                            .revisions
                            .cleanup_incomplete_promotion_candidate(record)
                            .await?;
                    }
                    Some(record)
                        if record.status == crate::skill_state::SkillRevisionStatus::Managed
                            && tree.package_id.as_ref() == Some(&record.package_id) => {}
                    Some(record) => {
                        record_tree_diagnostic(
                            backend,
                            "managed",
                            "managed_tree_binding_mismatch",
                            &tree.name,
                            Some(&record.revision_id),
                        )
                        .await?;
                    }
                    None => {
                        record_tree_diagnostic(
                            backend,
                            "managed",
                            "tree_only_managed",
                            &tree.name,
                            None,
                        )
                        .await?;
                    }
                }
            }
        }
    }

    for record in rows {
        if backend.revisions.revision_tree_exists(&record).await? {
            continue;
        }
        match record.status {
            crate::skill_state::SkillRevisionStatus::Staging => {
                backend
                    .state
                    .delete_staging_revision_record_if_matches(&record)
                    .await?;
            }
            crate::skill_state::SkillRevisionStatus::Managed => {
                record_tree_diagnostic(
                    backend,
                    "managed",
                    "row_only_managed",
                    &record.revision_id,
                    Some(&record.revision_id),
                )
                .await?;
            }
            crate::skill_state::SkillRevisionStatus::Quarantined => {
                record_tree_diagnostic(
                    backend,
                    "quarantine",
                    "row_only_quarantine",
                    &record.revision_id,
                    Some(&record.revision_id),
                )
                .await?;
            }
        }
    }

    for issue in backend.revisions.maintenance_issues() {
        let key = format!("store-issue:{}:{}", issue.revision_id, issue.operation);
        backend
            .state
            .record_maintenance_diagnostic_once(
                &key,
                Some(&issue.revision_id),
                "store",
                &issue.operation,
                serde_json::json!({"source": "process_local_carryover"}),
            )
            .await?;
    }
    backend.state.maintenance_diagnostic_count().await
}

async fn record_tree_diagnostic(
    backend: &crate::skill_manager::ManagedRuntimeBackend,
    area: &str,
    operation: &str,
    identity: &str,
    revision_id: Option<&str>,
) -> anyhow::Result<()> {
    let key = format!("tree:{area}:{operation}:{identity}");
    backend
        .state
        .record_maintenance_diagnostic_once(
            &key,
            revision_id,
            area,
            operation,
            serde_json::json!({"identity": identity, "ownership": "unproven"}),
        )
        .await?;
    Ok(())
}
