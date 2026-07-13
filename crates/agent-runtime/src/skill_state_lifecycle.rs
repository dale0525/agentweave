use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, SkillApprovalRecord, SkillApprovalStatus, SkillCircuitStateRecord,
    SkillInstallStatus, SkillInstallationRecord, SkillLayerRecord, SkillRevisionRecord,
    SkillStateBoundaryError, SkillStateStore,
};
use crate::skill_state_rows::{
    APPROVAL_COLUMNS, CIRCUIT_COLUMNS, INSTALLATION_COLUMNS, REVISION_COLUMNS, approval_from_row,
    circuit_from_row, installation_from_row, revision_from_row,
};
use chrono::{Duration, Utc};

pub(crate) enum LifecycleTarget<'a> {
    Rollback { revision: &'a SkillRevisionRecord },
    Disabled,
    Removed,
}

pub(crate) struct LifecycleApproval<'a> {
    pub approval_id: &'a str,
    pub approver_id: &'a str,
    pub operation: &'a str,
    pub expected_binding: &'a serde_json::Value,
}

pub(crate) struct ExactLifecyclePublication<'a> {
    pub actor_id: &'a str,
    pub operation: &'a str,
    pub package_id: &'a SkillPackageId,
    pub expected_installation: &'a SkillInstallationRecord,
    pub target: LifecycleTarget<'a>,
    pub approval: Option<LifecycleApproval<'a>>,
    pub previous_generation: u64,
    pub previous_members: serde_json::Value,
    pub generation: u64,
    pub members: serde_json::Value,
}

pub(crate) struct ExactSnapshotPublication<'a> {
    pub actor_id: &'a str,
    pub previous_generation: u64,
    pub previous_members: serde_json::Value,
    pub generation: u64,
    pub members: serde_json::Value,
    pub circuit_mutations: &'a [CircuitSnapshotMutation],
}

pub(crate) struct CircuitSnapshotMutation {
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub expected_circuit: SkillCircuitStateRecord,
    pub transition: CircuitSnapshotTransition,
    pub operation: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CircuitSnapshotTransition {
    Open,
    Consume,
}

impl SkillStateStore {
    pub(crate) async fn commit_exact_snapshot_publication(
        &self,
        input: ExactSnapshotPublication<'_>,
    ) -> anyhow::Result<()> {
        let generation = i64::try_from(input.generation)?;
        let previous_generation = i64::try_from(input.previous_generation)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            validate_circuit_expectations(&mut tx, input.circuit_mutations).await?;
            persist_snapshot_transition(
                &mut tx,
                previous_generation,
                &input.previous_members,
                generation,
                &input.members,
                &now,
            )
            .await?;
            for mutation in input.circuit_mutations {
                persist_circuit_omission_transition(
                    &mut tx,
                    &mutation.package_id,
                    &mutation.revision_id,
                    generation,
                    mutation.transition,
                    &now,
                )
                .await?;
                crate::skill_state::insert_audit(
                    &mut *tx,
                    input.actor_id,
                    mutation.operation,
                    &mutation.package_id,
                    Some(&mutation.revision_id),
                    "ok",
                    serde_json::json!({"generation": generation}),
                )
                .await?;
            }
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn create_removal_approval_unique(
        &self,
        input: NewSkillApproval,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.create_lifecycle_approval_unique(input, "remove").await
    }

    pub(crate) async fn create_lifecycle_approval_unique(
        &self,
        input: NewSkillApproval,
        operation: &'static str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let binding = input.binding.clone().ok_or_else(|| {
            SkillStateBoundaryError::InvalidInput(anyhow::anyhow!(
                "removal approval requires an exact binding"
            ))
        })?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let cleanup: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM skill_revision_cleanup WHERE revision_id = ? AND status = 'pending' LIMIT 1",
            )
            .bind(&input.revision_id)
            .fetch_optional(&mut *tx)
            .await?;
            if cleanup.is_some() {
                return Err(state_conflict(
                    "skill revision has a pending cleanup operation",
                ));
            }
            let select = format!(
                "SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE package_id = ? AND operation = ? AND status = 'pending'"
            );
            if let Some(row) = sqlx::query(&select)
                .bind(input.package_id.as_str())
                .bind(operation)
                .fetch_optional(&mut *tx)
                .await?
            {
                let approval = approval_from_row(&row)?;
                let stored: String = sqlx::query_scalar(
                    "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
                )
                .bind(&approval.approval_id)
                .fetch_one(&mut *tx)
                .await?;
                if approval.requested_by == input.requested_by
                    && serde_json::from_str::<serde_json::Value>(&stored)? == binding
                {
                    return Ok(approval);
                }
                return Err(state_conflict("a conflicting removal approval is pending"));
            }

            let approval_id = uuid::Uuid::new_v4().to_string();
            let now = Utc::now();
            sqlx::query(
                r#"INSERT INTO skill_approvals
                   (approval_id, package_id, revision_id, operation, requested_by, approved_by,
                    status, permission_diff, created_at, resolved_at)
                   VALUES (?, ?, ?, ?, ?, NULL, 'pending', ?, ?, NULL)"#,
            )
            .bind(&approval_id)
            .bind(input.package_id.as_str())
            .bind(&input.revision_id)
            .bind(operation)
            .bind(&input.requested_by)
            .bind(serde_json::to_string(&input.permission_diff)?)
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO skill_approval_bindings (approval_id, binding_json) VALUES (?, ?)",
            )
            .bind(&approval_id)
            .bind(serde_json::to_string(&binding)?)
            .execute(&mut *tx)
            .await?;
            Ok(SkillApprovalRecord {
                approval_id,
                package_id: input.package_id,
                revision_id: input.revision_id,
                operation: operation.into(),
                requested_by: input.requested_by,
                approved_by: None,
                status: SkillApprovalStatus::Pending,
                permission_diff: input.permission_diff,
                created_at: now,
                resolved_at: None,
            })
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn approval_binding_value(
        &self,
        approval_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        crate::skill_state_rows::validate_uuid_v4("approval_id", approval_id)
            .map_err(SkillStateBoundaryError::InvalidInput)?;
        let value: String = sqlx::query_scalar(
            "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
        )
        .bind(approval_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| state_conflict("approval binding is missing"))?;
        Ok(serde_json::from_str(&value)?)
    }

    pub(crate) async fn commit_exact_lifecycle_publication(
        &self,
        input: ExactLifecyclePublication<'_>,
    ) -> anyhow::Result<()> {
        let generation = i64::try_from(input.generation)?;
        let previous_generation = i64::try_from(input.previous_generation)?;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let installation_query = format!(
                "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?"
            );
            let current = sqlx::query(&installation_query)
                .bind(input.package_id.as_str())
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| installation_from_row(&row))
                .transpose()?
                .ok_or_else(|| state_conflict("managed installation is missing"))?;
            if current != *input.expected_installation
                || current.source_layer != SkillLayerRecord::Managed
            {
                return Err(state_conflict("managed installation changed"));
            }

            if let Some(approval) = &input.approval {
                let approval_query = format!(
                    "SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?"
                );
                let row = sqlx::query(&approval_query)
                    .bind(approval.approval_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| state_not_found("removal approval is missing"))?;
                let record = approval_from_row(&row)?;
                if record.status != SkillApprovalStatus::Pending
                    || record.operation != approval.operation
                    || record.package_id != *input.package_id
                    || record.requested_by == approval.approver_id
                {
                    return Err(state_conflict("removal approval conflicts with current state"));
                }
                let stored: String = sqlx::query_scalar(
                    "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
                )
                .bind(approval.approval_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| state_conflict("removal approval binding is missing"))?;
                if serde_json::from_str::<serde_json::Value>(&stored)?
                    != *approval.expected_binding
                {
                    return Err(state_conflict("removal approval binding is stale"));
                }
            }

            let (revision_id, enabled, status) = match input.target {
                LifecycleTarget::Rollback { revision } => {
                    let revision_query = format!(
                        "SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?"
                    );
                    let current_revision = sqlx::query(&revision_query)
                    .bind(&revision.revision_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(|row| revision_from_row(&row))
                    .transpose()?;
                    if current_revision.as_ref() != Some(revision)
                        || revision.package_id != *input.package_id
                    {
                        return Err(state_conflict("rollback revision binding is stale"));
                    }
                    let cleanup: Option<String> = sqlx::query_scalar(
                        "SELECT expected_json FROM skill_revision_cleanup WHERE revision_id = ? AND status = 'pending'",
                    )
                    .bind(&revision.revision_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let Some(cleanup) = cleanup {
                        if cleanup
                            == crate::skill_state_cleanup::cleanup_expectation(revision)?
                        {
                            return Err(state_conflict(
                                "rollback revision has a pending cleanup operation",
                            ));
                        }
                        return Err(state_conflict(
                            "rollback revision cleanup expectation is stale",
                        ));
                    }
                    (
                        Some(revision.revision_id.as_str()),
                        true,
                        SkillInstallStatus::Active,
                    )
                }
                LifecycleTarget::Disabled => (
                    current.active_revision_id.as_deref(),
                    false,
                    SkillInstallStatus::Disabled,
                ),
                LifecycleTarget::Removed => (
                    current.active_revision_id.as_deref(),
                    false,
                    SkillInstallStatus::Removed,
                ),
            };
            sqlx::query(
                "UPDATE skill_installations SET active_revision_id = ?, enabled = ?, install_status = ?, updated_at = ? WHERE package_id = ?",
            )
            .bind(revision_id)
            .bind(i64::from(enabled))
            .bind(status.as_str())
            .bind(&now_text)
            .bind(input.package_id.as_str())
            .execute(&mut *tx)
            .await?;

            if let Some(previous_revision) = current.active_revision_id.as_deref() {
                sqlx::query(
                    r#"INSERT INTO skill_revision_retention
                       (revision_id, package_id, reason, retain_until, created_at)
                       VALUES (?, ?, ?, ?, ?)
                       ON CONFLICT(revision_id) DO UPDATE SET
                         reason = excluded.reason, retain_until = excluded.retain_until"#,
                )
                .bind(previous_revision)
                .bind(input.package_id.as_str())
                .bind(input.operation)
                .bind((now + Duration::days(7)).to_rfc3339())
                .bind(&now_text)
                .execute(&mut *tx)
                .await?;
            }

            persist_snapshot_transition(
                &mut tx,
                previous_generation,
                &input.previous_members,
                generation,
                &input.members,
                &now_text,
            )
            .await?;

            if let Some(approval) = &input.approval {
                let changed = sqlx::query(
                    "UPDATE skill_approvals SET approved_by = ?, status = 'approved', resolved_at = ? WHERE approval_id = ? AND status = 'pending' AND requested_by != ?",
                )
                .bind(approval.approver_id)
                .bind(&now_text)
                .bind(approval.approval_id)
                .bind(approval.approver_id)
                .execute(&mut *tx)
                .await?;
                if changed.rows_affected() != 1 {
                    return Err(state_conflict("removal approval was already resolved"));
                }
            }
            crate::skill_state::insert_audit(
                &mut *tx,
                input.actor_id,
                input.operation,
                input.package_id,
                revision_id,
                "ok",
                serde_json::json!({"generation": generation}),
            )
            .await?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

async fn validate_circuit_expectations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    mutations: &[CircuitSnapshotMutation],
) -> anyhow::Result<()> {
    let query = format!("SELECT {CIRCUIT_COLUMNS} FROM skill_circuit_state WHERE revision_id = ?");
    for mutation in mutations {
        if mutation.expected_circuit.revision_id != mutation.revision_id {
            return Err(state_conflict("circuit mutation identity is inconsistent"));
        }
        let current = sqlx::query(&query)
            .bind(&mutation.revision_id)
            .fetch_optional(&mut **tx)
            .await?
            .map(|row| circuit_from_row(&row))
            .transpose()?;
        if current.as_ref() != Some(&mutation.expected_circuit) {
            return Err(state_conflict(
                "circuit state changed before snapshot publication",
            ));
        }
    }
    Ok(())
}

async fn persist_circuit_omission_transition(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    package_id: &SkillPackageId,
    revision_id: &str,
    generation: i64,
    transition: CircuitSnapshotTransition,
    now: &str,
) -> anyhow::Result<()> {
    let revision: Option<(String, String)> = sqlx::query_as(
        "SELECT package_id, lifecycle_status FROM skill_revisions WHERE revision_id = ?",
    )
    .bind(revision_id)
    .fetch_optional(&mut **tx)
    .await?;
    if revision.as_ref() != Some(&(package_id.as_str().to_string(), "managed".into())) {
        return Err(state_conflict("circuit omission revision identity changed"));
    }
    match transition {
        CircuitSnapshotTransition::Open => {
            let existing: Option<Option<i64>> = sqlx::query_scalar(
                "SELECT consumed_generation FROM skill_circuit_omissions WHERE revision_id = ?",
            )
            .bind(revision_id)
            .fetch_optional(&mut **tx)
            .await?;
            if matches!(existing, Some(None)) {
                return Err(state_conflict("circuit omission is already pending"));
            }
            sqlx::query(
                r#"INSERT INTO skill_circuit_omissions
                   (revision_id, package_id, omitted_generation, consumed_generation,
                    created_at, consumed_at)
                   VALUES (?, ?, ?, NULL, ?, NULL)
                   ON CONFLICT(revision_id) DO UPDATE SET
                     package_id = excluded.package_id,
                     omitted_generation = excluded.omitted_generation,
                     consumed_generation = NULL,
                     created_at = excluded.created_at,
                     consumed_at = NULL"#,
            )
            .bind(revision_id)
            .bind(package_id.as_str())
            .bind(generation)
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
        CircuitSnapshotTransition::Consume => {
            let consumed = sqlx::query(
                r#"UPDATE skill_circuit_omissions
                   SET consumed_generation = ?, consumed_at = ?
                   WHERE revision_id = ? AND package_id = ?
                     AND consumed_generation IS NULL AND omitted_generation < ?"#,
            )
            .bind(generation)
            .bind(now)
            .bind(revision_id)
            .bind(package_id.as_str())
            .bind(generation)
            .execute(&mut **tx)
            .await?;
            if consumed.rows_affected() != 1 {
                return Err(state_conflict("circuit omission is not pending"));
            }
        }
    }
    Ok(())
}

pub(crate) async fn persist_snapshot_transition(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    previous_generation: i64,
    previous_members: &serde_json::Value,
    generation: i64,
    members: &serde_json::Value,
    now: &str,
) -> anyhow::Result<()> {
    if generation
        != previous_generation
            .checked_add(1)
            .ok_or_else(|| state_conflict("snapshot generation overflow"))?
    {
        return Err(state_conflict("snapshot generation is not consecutive"));
    }
    let active: Option<(i64, String)> = sqlx::query_as(
        "SELECT generation, members_json FROM skill_snapshots WHERE status = 'active'",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some((active_generation, active_members)) = active else {
        return Err(state_conflict("authoritative active snapshot is missing"));
    };
    if active_generation != previous_generation
        || serde_json::from_str::<serde_json::Value>(&active_members)? != *previous_members
    {
        return Err(state_conflict("authoritative active snapshot changed"));
    }
    sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good' AND generation NOT IN (?, ?)")
        .bind(previous_generation)
        .bind(generation)
        .execute(&mut **tx)
        .await?;
    let demoted = sqlx::query("UPDATE skill_snapshots SET status = 'last_known_good' WHERE status = 'active' AND generation = ? AND members_json = ?")
        .bind(previous_generation)
        .bind(&active_members)
        .execute(&mut **tx)
        .await?;
    if demoted.rows_affected() != 1 {
        return Err(state_conflict("authoritative active snapshot changed"));
    }
    let inserted = sqlx::query(
        r#"INSERT INTO skill_snapshots
           (generation, status, members_json, created_at, activated_at)
           VALUES (?, 'active', ?, ?, ?)
           ON CONFLICT(generation) DO NOTHING"#,
    )
    .bind(generation)
    .bind(serde_json::to_string(members)?)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(state_conflict("snapshot generation already exists"));
    }
    Ok(())
}

fn state_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}

fn state_not_found(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::NotFound(anyhow::anyhow!(message)).into()
}
