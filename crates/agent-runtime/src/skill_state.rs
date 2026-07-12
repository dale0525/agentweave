use crate::skill_package::SkillPackageId;
use crate::skill_state_rows::{
    APPROVAL_COLUMNS, AUDIT_COLUMNS, CIRCUIT_COLUMNS, REVISION_COLUMNS, SNAPSHOT_COLUMNS,
    approval_from_row, audit_from_row, circuit_from_row, parse_json, revision_from_row,
    snapshot_from_row, validate_storage_path, validate_uuid_v4,
};
use crate::storage::Storage;
use anyhow::Context;
use chrono::Utc;
use serde_json::{Map, Value};
use sqlx::{Executor, Row, Sqlite, SqlitePool};
use uuid::Uuid;

pub use crate::skill_state_management::ManagedSkillInstallationView;
pub use crate::skill_state_rows::{
    NewSkillApproval, NewSkillRevision, SkillApprovalRecord, SkillApprovalStatus, SkillAuditRecord,
    SkillCircuitStateRecord, SkillInstallStatus, SkillInstallationRecord, SkillLayerRecord,
    SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion, SkillRevisionRecord,
    SkillRevisionStatus, SkillSnapshotRecord, SkillSnapshotStatus,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SkillStateBoundaryError {
    #[error("invalid skill state request")]
    InvalidInput(#[source] anyhow::Error),
    #[error("skill state resource not found")]
    NotFound(#[source] anyhow::Error),
    #[error("skill state conflicts with current state")]
    Conflict(#[source] anyhow::Error),
}

pub(crate) fn validate_revision_id(revision_id: &str) -> anyhow::Result<()> {
    validate_uuid_v4("revision_id", revision_id).map_err(SkillStateBoundaryError::InvalidInput)?;
    Ok(())
}

#[derive(Clone)]
pub struct SkillStateStore {
    storage: Storage,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    crate::skill_state_migration::migrate(pool).await
}

impl SkillStateStore {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        self.storage.pool()
    }

    pub fn allocate_revision_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// managed revision. Runtime authoring and staging flows must use
    /// [`Self::create_staging_revision_record`] followed by [`Self::promote_revision_record`].
    pub async fn create_revision(
        &self,
        input: NewSkillRevision,
    ) -> anyhow::Result<SkillRevisionRecord> {
        self.create_trusted_managed_revision_record(input).await
    }

    async fn create_trusted_managed_revision_record(
        &self,
        input: NewSkillRevision,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let revision_id = Self::allocate_revision_id();
        self.insert_revision(&revision_id, input, SkillRevisionStatus::Managed)
            .await
    }

    pub async fn create_staging_revision_record(
        &self,
        revision_id: &str,
        input: NewSkillRevision,
    ) -> anyhow::Result<SkillRevisionRecord> {
        self.insert_revision(revision_id, input, SkillRevisionStatus::Staging)
            .await
    }

    pub async fn create_quarantined_revision_record(
        &self,
        revision_id: &str,
        input: NewSkillRevision,
    ) -> anyhow::Result<SkillRevisionRecord> {
        self.insert_revision(revision_id, input, SkillRevisionStatus::Quarantined)
            .await
    }

    async fn insert_revision(
        &self,
        revision_id: &str,
        input: NewSkillRevision,
        status: SkillRevisionStatus,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_revision_id(revision_id)?;
        validate_storage_path(&input.storage_path)?;
        let created_at = Utc::now();
        let descriptor_json = serde_json::to_string(&input.descriptor_json)?;
        let validation_json = serde_json::to_string(&input.validation_json)?;
        sqlx::query(
            r#"INSERT INTO skill_revisions
               (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
                validation_json, created_by, created_at, lifecycle_status)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(revision_id)
        .bind(input.package_id.as_str())
        .bind(&input.version)
        .bind(&input.content_hash)
        .bind(&input.storage_path)
        .bind(descriptor_json)
        .bind(validation_json)
        .bind(&input.created_by)
        .bind(created_at.to_rfc3339())
        .bind(status.as_str())
        .execute(self.storage.pool())
        .await?;

        Ok(SkillRevisionRecord {
            revision_id: revision_id.to_string(),
            package_id: input.package_id,
            version: input.version,
            content_hash: input.content_hash,
            storage_path: input.storage_path,
            descriptor_json: input.descriptor_json,
            validation_json: input.validation_json,
            created_by: input.created_by,
            created_at,
            status,
        })
    }

    pub async fn get_revision(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<Option<SkillRevisionRecord>> {
        validate_revision_id(revision_id)?;
        let query = format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
        sqlx::query(&query)
            .bind(revision_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| revision_from_row(&row))
            .transpose()
    }

    pub async fn revision_validation(&self, revision_id: &str) -> anyhow::Result<Value> {
        validate_revision_id(revision_id)?;
        let value: Option<String> =
            sqlx::query_scalar("SELECT validation_json FROM skill_revisions WHERE revision_id = ?")
                .bind(revision_id)
                .fetch_optional(self.storage.pool())
                .await?;
        let value = value.with_context(|| format!("skill revision not found: {revision_id}"))?;
        parse_json("validation_json", &value)
    }

    pub async fn update_revision_validation(
        &self,
        revision_id: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        validate_revision_id(revision_id)?;
        let value = serde_json::to_string(&value)?;
        let result =
            sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
                .bind(value)
                .bind(revision_id)
                .execute(self.storage.pool())
                .await?;
        ensure_changed(result.rows_affected(), "skill revision", revision_id)
    }

    pub async fn refresh_staging_revision_metadata(
        &self,
        revision_id: &str,
        metadata: SkillRevisionMetadata,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_revision_id(revision_id)?;
        let descriptor_json = serde_json::to_string(&metadata.descriptor_json)?;
        let validation_json = serde_json::to_string(&metadata.validation_json)?;
        let query = format!(
            r#"UPDATE skill_revisions
               SET version = ?, content_hash = ?, descriptor_json = ?, validation_json = ?
               WHERE revision_id = ? AND lifecycle_status = 'staging'
               RETURNING {REVISION_COLUMNS}"#
        );
        let updated = sqlx::query(&query)
            .bind(&metadata.version)
            .bind(&metadata.content_hash)
            .bind(descriptor_json)
            .bind(validation_json)
            .bind(revision_id)
            .fetch_optional(self.storage.pool())
            .await?;
        if let Some(row) = updated {
            return revision_from_row(&row);
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle_status FROM skill_revisions WHERE revision_id = ?",
        )
        .bind(revision_id)
        .fetch_optional(self.storage.pool())
        .await?;
        let status = status.with_context(|| format!("skill revision not found: {revision_id}"))?;
        let status = SkillRevisionStatus::parse(&status)?;
        anyhow::bail!(
            "skill revision metadata cannot be refreshed from {}: {revision_id}",
            status.as_str()
        )
    }

    pub async fn promote_revision_record(
        &self,
        revision_id: &str,
        managed_storage_path: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_revision_id(revision_id)?;
        validate_storage_path(managed_storage_path)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let query = format!(
                r#"UPDATE skill_revisions
                   SET storage_path = ?, lifecycle_status = 'managed'
                   WHERE revision_id = ? AND lifecycle_status = 'staging'
                   RETURNING {REVISION_COLUMNS}"#
            );
            let updated = sqlx::query(&query)
                .bind(managed_storage_path)
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?;
            if let Some(row) = updated {
                return revision_from_row(&row);
            }
            promotion_rejection(&mut *tx, revision_id).await
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn promote_revision_record_with_metadata(
        &self,
        revision_id: &str,
        promotion: SkillRevisionPromotion,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_revision_id(revision_id)?;
        validate_storage_path(&promotion.storage_path)?;
        let descriptor_json = serde_json::to_string(&promotion.descriptor_json)?;
        let validation_json = serde_json::to_string(&promotion.validation_json)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let query = format!(
                r#"UPDATE skill_revisions
                   SET version = ?, content_hash = ?, storage_path = ?, descriptor_json = ?,
                       validation_json = ?, lifecycle_status = 'managed'
                   WHERE revision_id = ? AND lifecycle_status = 'staging'
                   RETURNING {REVISION_COLUMNS}"#
            );
            let updated = sqlx::query(&query)
                .bind(&promotion.version)
                .bind(&promotion.content_hash)
                .bind(&promotion.storage_path)
                .bind(&descriptor_json)
                .bind(&validation_json)
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?;
            if let Some(row) = updated {
                return revision_from_row(&row);
            }
            promotion_rejection(&mut *tx, revision_id).await
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn activate_revision(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        layer: SkillLayerRecord,
        actor_id: &str,
    ) -> anyhow::Result<()> {
        self.record_revision_activation(package_id, revision_id, layer, actor_id)
            .await
    }

    pub async fn record_revision_activation(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        layer: SkillLayerRecord,
        actor_id: &str,
    ) -> anyhow::Result<()> {
        validate_revision_id(revision_id)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.storage.pool().begin().await?;
        let result = async {
            let activated = sqlx::query_scalar::<_, String>(
                r#"INSERT INTO skill_installations
                   (package_id, source_layer, active_revision_id, enabled, trust_level,
                    install_status, installed_at, updated_at)
                   SELECT package_id, ?, revision_id, 1, 'approved', 'active', ?, ?
                   FROM skill_revisions
                   WHERE revision_id = ? AND package_id = ? AND lifecycle_status = 'managed'
                   ON CONFLICT(package_id) DO UPDATE SET
                     source_layer = excluded.source_layer,
                     active_revision_id = excluded.active_revision_id,
                     enabled = 1,
                     trust_level = excluded.trust_level,
                     install_status = 'active',
                     updated_at = excluded.updated_at
                   RETURNING package_id"#,
            )
            .bind(layer.as_str())
            .bind(&now)
            .bind(&now)
            .bind(revision_id)
            .bind(package_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            if activated.is_none() {
                return Err(activation_rejection(&mut *tx, package_id, revision_id).await?);
            }
            insert_audit(
                &mut *tx,
                actor_id,
                "activate_revision",
                package_id,
                Some(revision_id),
                "ok",
                Value::Object(Map::new()),
            )
            .await?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn create_approval(
        &self,
        input: NewSkillApproval,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval_id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let permission_diff = serde_json::to_string(&input.permission_diff)?;
        let mut tx = self.storage.pool().begin().await?;
        sqlx::query(
            r#"INSERT INTO skill_approvals
               (approval_id, package_id, revision_id, operation, requested_by, approved_by,
                status, permission_diff, created_at, resolved_at)
               VALUES (?, ?, ?, ?, ?, NULL, 'pending', ?, ?, NULL)"#,
        )
        .bind(&approval_id)
        .bind(input.package_id.as_str())
        .bind(&input.revision_id)
        .bind(&input.operation)
        .bind(&input.requested_by)
        .bind(permission_diff)
        .bind(created_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if let Some(binding) = &input.binding {
            sqlx::query(
                "INSERT INTO skill_approval_bindings (approval_id, binding_json) VALUES (?, ?)",
            )
            .bind(&approval_id)
            .bind(serde_json::to_string(binding)?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        Ok(SkillApprovalRecord {
            approval_id,
            package_id: input.package_id,
            revision_id: input.revision_id,
            operation: input.operation,
            requested_by: input.requested_by,
            approved_by: None,
            status: SkillApprovalStatus::Pending,
            permission_diff: input.permission_diff,
            created_at,
            resolved_at: None,
        })
    }

    pub async fn get_approval(
        &self,
        approval_id: &str,
    ) -> anyhow::Result<Option<SkillApprovalRecord>> {
        validate_uuid_v4("approval_id", approval_id)
            .map_err(SkillStateBoundaryError::InvalidInput)?;
        let query = format!("SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?");
        sqlx::query(&query)
            .bind(approval_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| approval_from_row(&row))
            .transpose()
    }

    pub async fn approve(
        &self,
        approval_id: &str,
        actor_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.resolve_approval(approval_id, actor_id, SkillApprovalStatus::Approved)
            .await
    }

    pub async fn reject(
        &self,
        approval_id: &str,
        actor_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.resolve_approval(approval_id, actor_id, SkillApprovalStatus::Rejected)
            .await
    }

    async fn resolve_approval(
        &self,
        approval_id: &str,
        actor_id: &str,
        target: SkillApprovalStatus,
    ) -> anyhow::Result<SkillApprovalRecord> {
        validate_uuid_v4("approval_id", approval_id)
            .map_err(SkillStateBoundaryError::InvalidInput)?;
        let mut tx = self.storage.pool().begin().await?;
        let result = async {
            let resolved_at = Utc::now().to_rfc3339();
            let updated = match target {
                SkillApprovalStatus::Approved => {
                    let query = format!(
                        r#"UPDATE skill_approvals
                           SET approved_by = ?, status = 'approved', resolved_at = ?
                           WHERE approval_id = ? AND status = 'pending' AND requested_by != ?
                           RETURNING {APPROVAL_COLUMNS}"#
                    );
                    sqlx::query(&query)
                        .bind(actor_id)
                        .bind(&resolved_at)
                        .bind(approval_id)
                        .bind(actor_id)
                        .fetch_optional(&mut *tx)
                        .await?
                }
                SkillApprovalStatus::Rejected => {
                    let query = format!(
                        r#"UPDATE skill_approvals
                           SET approved_by = ?, status = 'rejected', resolved_at = ?
                           WHERE approval_id = ? AND status = 'pending'
                           RETURNING {APPROVAL_COLUMNS}"#
                    );
                    sqlx::query(&query)
                        .bind(actor_id)
                        .bind(&resolved_at)
                        .bind(approval_id)
                        .fetch_optional(&mut *tx)
                        .await?
                }
                SkillApprovalStatus::Pending => {
                    anyhow::bail!("cannot resolve approval to pending")
                }
            };
            if let Some(row) = updated {
                return approval_from_row(&row);
            }

            let select =
                format!("SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?");
            let row = sqlx::query(&select)
                .bind(approval_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| {
                    SkillStateBoundaryError::NotFound(anyhow::anyhow!("skill approval not found"))
                })?;
            let approval = approval_from_row(&row)?;
            if approval.status != SkillApprovalStatus::Pending {
                return Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                    "skill approval already resolved"
                ))
                .into());
            }
            if target == SkillApprovalStatus::Approved && approval.requested_by == actor_id {
                return Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                    "requester cannot approve their own request"
                ))
                .into());
            }
            Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                "skill approval could not be resolved"
            ))
            .into())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn list_audit(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Vec<SkillAuditRecord>> {
        let query = format!(
            "SELECT {AUDIT_COLUMNS} FROM skill_audit_log WHERE package_id = ? ORDER BY created_at, id"
        );
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_all(self.storage.pool())
            .await?
            .iter()
            .map(audit_from_row)
            .collect()
    }

    pub async fn record_revision_diagnostic(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        operation: &str,
        metadata: Value,
    ) -> anyhow::Result<()> {
        validate_revision_id(revision_id)?;
        if operation.trim().is_empty() {
            anyhow::bail!("diagnostic operation cannot be empty");
        }
        insert_audit(
            self.storage.pool(),
            "managed-skill-source",
            operation,
            package_id,
            Some(revision_id),
            "error",
            metadata,
        )
        .await
    }

    pub async fn mark_revision_quarantined(
        &self,
        revision_id: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.quarantine_revision_internal(revision_id, None, reason)
            .await?;
        Ok(())
    }

    pub async fn quarantine_revision_record(
        &self,
        revision_id: &str,
        quarantined_storage_path: &str,
        reason: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_storage_path(quarantined_storage_path)?;
        self.quarantine_revision_internal(revision_id, Some(quarantined_storage_path), reason)
            .await
    }

    pub async fn quarantine_revision_record_cas(
        &self,
        revision_id: &str,
        quarantined_storage_path: &str,
        reason: &str,
        expected: SkillRevisionExpectation,
        replacement_metadata: Option<SkillRevisionMetadata>,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_storage_path(quarantined_storage_path)?;
        validate_storage_path(&expected.storage_path)?;
        self.quarantine_revision_internal_cas(
            revision_id,
            Some(quarantined_storage_path),
            reason,
            Some(expected),
            replacement_metadata,
        )
        .await
    }

    async fn quarantine_revision_internal(
        &self,
        revision_id: &str,
        storage_path: Option<&str>,
        reason: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        self.quarantine_revision_internal_cas(revision_id, storage_path, reason, None, None)
            .await
    }

    async fn quarantine_revision_internal_cas(
        &self,
        revision_id: &str,
        storage_path: Option<&str>,
        reason: &str,
        expected: Option<SkillRevisionExpectation>,
        replacement_metadata: Option<SkillRevisionMetadata>,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_revision_id(revision_id)?;
        let now = Utc::now();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let select =
                format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
            let row = sqlx::query(&select)
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| {
                    SkillStateBoundaryError::NotFound(anyhow::anyhow!(
                        "skill revision not found"
                    ))
                })?;
            let revision = revision_from_row(&row)?;
            if !matches!(
                revision.status,
                SkillRevisionStatus::Staging | SkillRevisionStatus::Managed
            ) {
                return Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                    "skill revision lifecycle changed before quarantine"
                ))
                .into());
            }
            if let Some(expected) = &expected
                && (revision.status != expected.status
                    || revision.version != expected.version
                    || revision.content_hash != expected.content_hash
                    || revision.storage_path != expected.storage_path
                    || revision.descriptor_json != expected.descriptor_json
                    || revision.validation_json != expected.validation_json)
            {
                return Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                    "skill revision changed since operation observation"
                ))
                .into());
            }
            let (version, content_hash, descriptor_json, validation_json) =
                if let Some(metadata) = replacement_metadata {
                    (
                        metadata.version,
                        metadata.content_hash,
                        serde_json::to_string(&metadata.descriptor_json)?,
                        metadata.validation_json,
                    )
                } else {
                    (
                        revision.version.clone(),
                        revision.content_hash.clone(),
                        serde_json::to_string(&revision.descriptor_json)?,
                        revision.validation_json.clone(),
                    )
                };
            let mut validation = match validation_json {
                Value::Object(map) => map,
                value => Map::from_iter([("previousValidation".into(), value)]),
            };
            validation.insert("quarantined".into(), Value::Bool(true));
            validation.insert("quarantineReason".into(), Value::String(reason.into()));
            validation.insert("quarantinedAt".into(), Value::String(now.to_rfc3339()));
            let validation = serde_json::to_string(&Value::Object(validation))?;
            let storage_path = storage_path.unwrap_or(&revision.storage_path);
            let result = if let Some(expected) = &expected {
                let expected_descriptor_json =
                    serde_json::to_string(&expected.descriptor_json)?;
                let expected_validation_json =
                    serde_json::to_string(&expected.validation_json)?;
                sqlx::query(
                    r#"UPDATE skill_revisions
                       SET version = ?, content_hash = ?, descriptor_json = ?, storage_path = ?,
                           lifecycle_status = 'quarantined', validation_json = ?
                       WHERE revision_id = ? AND lifecycle_status = ? AND version = ?
                         AND content_hash = ? AND storage_path = ?
                         AND descriptor_json = ? AND validation_json = ?"#,
                )
                .bind(&version)
                .bind(&content_hash)
                .bind(&descriptor_json)
                .bind(storage_path)
                .bind(&validation)
                .bind(revision_id)
                .bind(expected.status.as_str())
                .bind(&expected.version)
                .bind(&expected.content_hash)
                .bind(&expected.storage_path)
                .bind(expected_descriptor_json)
                .bind(expected_validation_json)
                .execute(&mut *tx)
                .await?
            } else {
                sqlx::query(
                    r#"UPDATE skill_revisions
                       SET version = ?, content_hash = ?, descriptor_json = ?, storage_path = ?,
                           lifecycle_status = 'quarantined', validation_json = ?
                       WHERE revision_id = ? AND lifecycle_status IN ('staging', 'managed')"#,
                )
                .bind(&version)
                .bind(&content_hash)
                .bind(&descriptor_json)
                .bind(storage_path)
                .bind(&validation)
                .bind(revision_id)
                .execute(&mut *tx)
                .await?
            };
            if result.rows_affected() == 0 && expected.is_some() {
                return Err(SkillStateBoundaryError::Conflict(anyhow::anyhow!(
                    "skill revision changed since operation observation"
                ))
                .into());
            }
            if result.rows_affected() == 0 {
                return Err(SkillStateBoundaryError::NotFound(anyhow::anyhow!(
                    "skill revision not found"
                ))
                .into());
            }
            sqlx::query(
                r#"UPDATE skill_installations
                   SET active_revision_id = NULL, enabled = 0, install_status = 'quarantined', updated_at = ?
                   WHERE package_id = ? AND active_revision_id = ?"#,
            )
            .bind(now.to_rfc3339())
            .bind(revision.package_id.as_str())
            .bind(revision_id)
            .execute(&mut *tx)
            .await?;
            insert_audit(
                &mut *tx,
                "system",
                "mark_revision_quarantined",
                &revision.package_id,
                Some(revision_id),
                "ok",
                serde_json::json!({"reason": reason}),
            )
            .await?;
            let row = sqlx::query(&select)
                .bind(revision_id)
                .fetch_one(&mut *tx)
                .await?;
            revision_from_row(&row)
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn record_snapshot_candidate(
        &self,
        generation: u64,
        members: Value,
    ) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let members = serde_json::to_string(&members)?;
        sqlx::query(
            r#"INSERT INTO skill_snapshots
               (generation, status, members_json, created_at, activated_at)
               VALUES (?, 'candidate', ?, ?, NULL)"#,
        )
        .bind(generation)
        .bind(members)
        .bind(Utc::now().to_rfc3339())
        .execute(self.storage.pool())
        .await?;
        Ok(())
    }

    pub async fn get_snapshot(
        &self,
        generation: u64,
    ) -> anyhow::Result<Option<SkillSnapshotRecord>> {
        let generation = sqlite_generation(generation)?;
        let query = format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
        sqlx::query(&query)
            .bind(generation)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| snapshot_from_row(&row))
            .transpose()
    }

    pub async fn mark_snapshot_active(&self, generation: u64) -> anyhow::Result<()> {
        self.record_snapshot_activation(generation).await
    }

    pub async fn record_snapshot_activation(&self, generation: u64) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let query =
                format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
            let row = sqlx::query(&query)
                .bind(generation)
                .fetch_optional(&mut *tx)
                .await?
                .with_context(|| format!("skill snapshot not found: {generation}"))?;
            let target = snapshot_from_row(&row)?;
            if matches!(
                target.status,
                SkillSnapshotStatus::Active | SkillSnapshotStatus::LastKnownGood
            ) {
                return Ok(());
            }
            sqlx::query(
                "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'active' AND generation != ?",
            )
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            let updated = sqlx::query(
                "UPDATE skill_snapshots SET status = 'active', activated_at = ? WHERE generation = ?",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            ensure_changed(
                updated.rows_affected(),
                "skill snapshot",
                &generation.to_string(),
            )?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn mark_snapshot_last_known_good(&self, generation: u64) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let query =
                format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
            let row = sqlx::query(&query)
                .bind(generation)
                .fetch_optional(&mut *tx)
                .await?
                .with_context(|| format!("skill snapshot not found: {generation}"))?;
            let target = snapshot_from_row(&row)?;
            match target.status {
                SkillSnapshotStatus::LastKnownGood => return Ok(()),
                SkillSnapshotStatus::Candidate => anyhow::bail!(
                    "skill snapshot must be active before becoming last known good: {generation}"
                ),
                SkillSnapshotStatus::Active => {}
            }
            sqlx::query(
                "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good'",
            )
            .execute(&mut *tx)
            .await?;
            let updated = sqlx::query(
                "UPDATE skill_snapshots SET status = 'last_known_good' WHERE generation = ? AND status = 'active'",
            )
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            ensure_changed(
                updated.rows_affected(),
                "active skill snapshot",
                &generation.to_string(),
            )?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn get_circuit_state(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<Option<SkillCircuitStateRecord>> {
        validate_revision_id(revision_id)?;
        let query =
            format!("SELECT {CIRCUIT_COLUMNS} FROM skill_circuit_state WHERE revision_id = ?");
        sqlx::query(&query)
            .bind(revision_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| circuit_from_row(&row))
            .transpose()
    }
}

pub(crate) async fn insert_audit<'e, E>(
    executor: E,
    actor_id: &str,
    operation: &str,
    package_id: &SkillPackageId,
    revision_id: Option<&str>,
    result: &str,
    metadata: Value,
) -> anyhow::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        r#"INSERT INTO skill_audit_log
           (id, actor_id, operation, package_id, revision_id, result, metadata_json, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor_id)
    .bind(operation)
    .bind(package_id.as_str())
    .bind(revision_id)
    .bind(result)
    .bind(serde_json::to_string(&metadata)?)
    .bind(Utc::now().to_rfc3339())
    .execute(executor)
    .await?;
    Ok(())
}

async fn promotion_rejection<'e, E>(
    executor: E,
    revision_id: &str,
) -> anyhow::Result<SkillRevisionRecord>
where
    E: Executor<'e, Database = Sqlite>,
{
    let status: Option<String> =
        sqlx::query_scalar("SELECT lifecycle_status FROM skill_revisions WHERE revision_id = ?")
            .bind(revision_id)
            .fetch_optional(executor)
            .await?;
    let status = status.with_context(|| format!("skill revision not found: {revision_id}"))?;
    let status = SkillRevisionStatus::parse(&status)?;
    anyhow::bail!(
        "skill revision cannot be promoted from {}: {revision_id}",
        status.as_str()
    )
}

async fn activation_rejection<'e, E>(
    executor: E,
    package_id: &SkillPackageId,
    revision_id: &str,
) -> anyhow::Result<anyhow::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        "SELECT package_id, lifecycle_status FROM skill_revisions WHERE revision_id = ?",
    )
    .bind(revision_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else {
        return Ok(anyhow::anyhow!("skill revision not found: {revision_id}"));
    };
    let stored_package: String = row.try_get("package_id")?;
    let stored_package = SkillPackageId::parse(&stored_package)?;
    if &stored_package != package_id {
        return Ok(anyhow::anyhow!(
            "skill revision {revision_id} belongs to {}, not {}",
            stored_package.as_str(),
            package_id.as_str()
        ));
    }
    let status: String = row.try_get("lifecycle_status")?;
    let status = SkillRevisionStatus::parse(&status)?;
    Ok(anyhow::anyhow!(
        "skill revision is not activatable in {} state",
        status.as_str()
    ))
}

fn sqlite_generation(generation: u64) -> anyhow::Result<i64> {
    i64::try_from(generation).context("snapshot generation exceeds SQLite INTEGER range")
}

fn ensure_changed(rows: u64, entity: &str, id: &str) -> anyhow::Result<()> {
    if rows == 0 {
        anyhow::bail!("{entity} not found: {id}");
    }
    Ok(())
}
