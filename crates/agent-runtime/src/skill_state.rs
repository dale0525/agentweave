use crate::skill_package::SkillPackageId;
use crate::skill_state_rows::{
    APPROVAL_COLUMNS, AUDIT_COLUMNS, CIRCUIT_COLUMNS, INSTALLATION_COLUMNS, REVISION_COLUMNS,
    SNAPSHOT_COLUMNS, approval_from_row, audit_from_row, circuit_from_row, installation_from_row,
    parse_json, revision_from_row, snapshot_from_row, validate_uuid_v4,
};
use crate::storage::Storage;
use anyhow::Context;
use chrono::Utc;
use serde_json::{Map, Value};
use sqlx::{Executor, Row, Sqlite, SqlitePool};
use uuid::Uuid;

pub use crate::skill_state_rows::{
    NewSkillApproval, NewSkillRevision, SkillApprovalRecord, SkillApprovalStatus, SkillAuditRecord,
    SkillCircuitStateRecord, SkillInstallStatus, SkillInstallationRecord, SkillLayerRecord,
    SkillRevisionRecord, SkillRevisionStatus, SkillSnapshotRecord, SkillSnapshotStatus,
};

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

    pub fn allocate_revision_id() -> String {
        Uuid::new_v4().to_string()
    }

    pub async fn create_revision(
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

    async fn insert_revision(
        &self,
        revision_id: &str,
        input: NewSkillRevision,
        status: SkillRevisionStatus,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
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
        validate_uuid_v4("revision_id", revision_id)?;
        let query = format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
        sqlx::query(&query)
            .bind(revision_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| revision_from_row(&row))
            .transpose()
    }

    pub async fn revision_validation(&self, revision_id: &str) -> anyhow::Result<Value> {
        validate_uuid_v4("revision_id", revision_id)?;
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
        validate_uuid_v4("revision_id", revision_id)?;
        let value = serde_json::to_string(&value)?;
        let result =
            sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
                .bind(value)
                .bind(revision_id)
                .execute(self.storage.pool())
                .await?;
        ensure_changed(result.rows_affected(), "skill revision", revision_id)
    }

    pub async fn promote_revision_record(
        &self,
        revision_id: &str,
        managed_storage_path: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        validate_storage_path(managed_storage_path)?;
        let mut tx = self.storage.pool().begin().await?;
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
            let revision = revision_from_row(&row)?;
            tx.commit().await?;
            return Ok(revision);
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle_status FROM skill_revisions WHERE revision_id = ?",
        )
        .bind(revision_id)
        .fetch_optional(&mut *tx)
        .await?;
        let status = status.with_context(|| format!("skill revision not found: {revision_id}"))?;
        let status = SkillRevisionStatus::parse(&status)?;
        anyhow::bail!(
            "skill revision cannot be promoted from {}: {revision_id}",
            status.as_str()
        )
    }

    pub async fn get_installation(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Option<SkillInstallationRecord>> {
        let query =
            format!("SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?");
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| installation_from_row(&row))
            .transpose()
    }

    pub async fn list_active_installations(&self) -> anyhow::Result<Vec<SkillInstallationRecord>> {
        let query = format!(
            "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE enabled = 1 AND install_status = 'active' ORDER BY package_id"
        );
        sqlx::query(&query)
            .fetch_all(self.storage.pool())
            .await?
            .iter()
            .map(installation_from_row)
            .collect()
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
        validate_uuid_v4("revision_id", revision_id)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.storage.pool().begin().await?;
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
            let error = activation_rejection(&mut *tx, package_id, revision_id).await;
            tx.rollback().await?;
            return Err(error?);
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
        tx.commit().await?;
        Ok(())
    }

    pub async fn create_approval(
        &self,
        input: NewSkillApproval,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval_id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let permission_diff = serde_json::to_string(&input.permission_diff)?;
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
        .execute(self.storage.pool())
        .await?;

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
        validate_uuid_v4("approval_id", approval_id)?;
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
        validate_uuid_v4("approval_id", approval_id)?;
        let mut tx = self.storage.pool().begin().await?;
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
            SkillApprovalStatus::Pending => anyhow::bail!("cannot resolve approval to pending"),
        };
        if let Some(row) = updated {
            let approval = approval_from_row(&row)?;
            tx.commit().await?;
            return Ok(approval);
        }

        let select =
            format!("SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?");
        let row = sqlx::query(&select)
            .bind(approval_id)
            .fetch_optional(&mut *tx)
            .await?
            .with_context(|| format!("skill approval not found: {approval_id}"))?;
        let approval = approval_from_row(&row)?;
        if approval.status != SkillApprovalStatus::Pending {
            anyhow::bail!("skill approval already resolved: {approval_id}");
        }
        if target == SkillApprovalStatus::Approved && approval.requested_by == actor_id {
            anyhow::bail!("requester cannot approve their own request");
        }
        anyhow::bail!("skill approval could not be resolved: {approval_id}")
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

    async fn quarantine_revision_internal(
        &self,
        revision_id: &str,
        storage_path: Option<&str>,
        reason: &str,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        let now = Utc::now();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.storage.pool()).await?;
        let result = async {
            let select =
                format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
            let row = sqlx::query(&select)
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?
                .with_context(|| format!("skill revision not found: {revision_id}"))?;
            let revision = revision_from_row(&row)?;
            if !matches!(
                revision.status,
                SkillRevisionStatus::Staging | SkillRevisionStatus::Managed
            ) {
                anyhow::bail!(
                    "skill revision cannot be quarantined from {} state",
                    revision.status.as_str()
                );
            }
            let mut validation = match revision.validation_json {
                Value::Object(map) => map,
                value => Map::from_iter([("previousValidation".into(), value)]),
            };
            validation.insert("quarantined".into(), Value::Bool(true));
            validation.insert("quarantineReason".into(), Value::String(reason.into()));
            validation.insert("quarantinedAt".into(), Value::String(now.to_rfc3339()));
            let validation = serde_json::to_string(&Value::Object(validation))?;
            let storage_path = storage_path.unwrap_or(&revision.storage_path);
            let result = sqlx::query(
                r#"UPDATE skill_revisions
                   SET storage_path = ?, lifecycle_status = 'quarantined', validation_json = ?
                   WHERE revision_id = ? AND lifecycle_status IN ('staging', 'managed')"#,
            )
            .bind(storage_path)
            .bind(validation)
            .bind(revision_id)
            .execute(&mut *tx)
            .await?;
            ensure_changed(result.rows_affected(), "skill revision", revision_id)?;
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
        let mut tx = self.storage.pool().begin().await?;
        let query = format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
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
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query(
            "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'active' AND generation != ?",
        )
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "UPDATE skill_snapshots SET status = 'active', activated_at = ? WHERE generation = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        ensure_changed(
            result.rows_affected(),
            "skill snapshot",
            &generation.to_string(),
        )?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_snapshot_last_known_good(&self, generation: u64) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let mut tx = self.storage.pool().begin().await?;
        let query = format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
        let row = sqlx::query(&query)
            .bind(generation)
            .fetch_optional(&mut *tx)
            .await?
            .with_context(|| format!("skill snapshot not found: {generation}"))?;
        let target = snapshot_from_row(&row)?;
        if target.status == SkillSnapshotStatus::LastKnownGood {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE generation = ?")
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good'",
        )
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "UPDATE skill_snapshots SET status = 'last_known_good' WHERE generation = ?",
        )
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        ensure_changed(
            result.rows_affected(),
            "skill snapshot",
            &generation.to_string(),
        )?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_circuit_state(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<Option<SkillCircuitStateRecord>> {
        validate_uuid_v4("revision_id", revision_id)?;
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

async fn insert_audit<'e, E>(
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

fn validate_storage_path(path: &str) -> anyhow::Result<()> {
    if path.trim().is_empty() {
        anyhow::bail!("revision storage path cannot be empty");
    }
    Ok(())
}
