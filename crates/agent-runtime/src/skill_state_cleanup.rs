use crate::skill_state::{
    SkillRevisionExpectation, SkillRevisionRecord, SkillRevisionStatus, SkillSnapshotRecord,
    SkillStateBoundaryError, SkillStateStore,
};
use crate::skill_state_rows::{
    REVISION_COLUMNS, SNAPSHOT_COLUMNS, revision_from_row, snapshot_from_row,
};
use chrono::Utc;
use serde_json::json;
use std::collections::BTreeSet;

impl SkillStateStore {
    pub(crate) async fn list_managed_revisions(&self) -> anyhow::Result<Vec<SkillRevisionRecord>> {
        let query = format!(
            "SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE lifecycle_status = 'managed' ORDER BY package_id, created_at, revision_id"
        );
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(revision_from_row)
            .collect()
    }

    pub(crate) async fn list_snapshot_records(&self) -> anyhow::Result<Vec<SkillSnapshotRecord>> {
        let query = format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots ORDER BY generation");
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(snapshot_from_row)
            .collect()
    }

    pub(crate) async fn delete_historical_snapshot_candidate_cas(
        &self,
        snapshot: &SkillSnapshotRecord,
    ) -> anyhow::Result<bool> {
        let generation = i64::try_from(snapshot.generation)?;
        let changed = sqlx::query(
            r#"DELETE FROM skill_snapshots
               WHERE generation = ? AND status = 'candidate' AND members_json = ?"#,
        )
        .bind(generation)
        .bind(serde_json::to_string(&snapshot.members_json)?)
        .execute(self.pool())
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub(crate) async fn lifecycle_protected_revision_ids(
        &self,
    ) -> anyhow::Result<BTreeSet<String>> {
        let mut protected = BTreeSet::new();
        let installations: Vec<String> = sqlx::query_scalar(
            r#"SELECT active_revision_id FROM skill_installations
               WHERE active_revision_id IS NOT NULL AND install_status != 'removed'"#,
        )
        .fetch_all(self.pool())
        .await?;
        protected.extend(installations);
        let approvals: Vec<String> =
            sqlx::query_scalar("SELECT revision_id FROM skill_approvals WHERE status = 'pending'")
                .fetch_all(self.pool())
                .await?;
        protected.extend(approvals);
        let retained: Vec<String> = sqlx::query_scalar(
            "SELECT revision_id FROM skill_revision_retention WHERE retain_until > ?",
        )
        .bind(Utc::now().to_rfc3339())
        .fetch_all(self.pool())
        .await?;
        protected.extend(retained);
        Ok(protected)
    }

    pub(crate) async fn prepare_revision_cleanup(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<bool> {
        if record.status != SkillRevisionStatus::Managed {
            return Err(state_conflict("only managed revisions can be cleaned up"));
        }
        let expected = cleanup_expectation(record)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let query =
                format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
            let current = sqlx::query(&query)
                .bind(&record.revision_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| revision_from_row(&row))
                .transpose()?
                .ok_or_else(|| state_conflict("cleanup revision disappeared"))?;
            if current != *record {
                return Err(state_conflict("cleanup revision changed"));
            }
            if revision_is_durably_protected(&mut tx, &record.revision_id).await? {
                return Ok(false);
            }
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT expected_json FROM skill_revision_cleanup WHERE revision_id = ?",
            )
            .bind(&record.revision_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(existing) = existing {
                if existing == expected {
                    return Ok(true);
                }
                return Err(state_conflict("cleanup job conflicts with revision state"));
            }
            sqlx::query(
                r#"INSERT INTO skill_revision_cleanup
                   (revision_id, package_id, expected_json, status, created_at)
                   VALUES (?, ?, ?, 'pending', ?)"#,
            )
            .bind(&record.revision_id)
            .bind(record.package_id.as_str())
            .bind(expected)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await?;
            Ok(true)
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn finish_revision_cleanup(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<()> {
        let expected = cleanup_expectation(record)?;
        let expectation = SkillRevisionExpectation::from(record);
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let pending: Option<String> = sqlx::query_scalar(
                "SELECT expected_json FROM skill_revision_cleanup WHERE revision_id = ? AND status = 'pending'",
            )
            .bind(&record.revision_id)
            .fetch_optional(&mut *tx)
            .await?;
            if pending.as_deref() != Some(expected.as_str()) {
                return Err(state_conflict("cleanup job is missing or stale"));
            }
            sqlx::query("DELETE FROM skill_circuit_state WHERE revision_id = ?")
                .bind(&record.revision_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM skill_revision_retention WHERE revision_id = ?")
                .bind(&record.revision_id)
                .execute(&mut *tx)
                .await?;
            let changed = sqlx::query(
                r#"DELETE FROM skill_revisions WHERE revision_id = ? AND package_id = ?
                   AND version = ? AND content_hash = ? AND storage_path = ?
                   AND descriptor_json = ? AND validation_json = ?
                   AND lifecycle_status = 'managed'"#,
            )
            .bind(&record.revision_id)
            .bind(record.package_id.as_str())
            .bind(&expectation.version)
            .bind(&expectation.content_hash)
            .bind(&expectation.storage_path)
            .bind(serde_json::to_string(&expectation.descriptor_json)?)
            .bind(serde_json::to_string(&expectation.validation_json)?)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(state_conflict("cleanup revision CAS failed"));
            }
            crate::skill_state::insert_audit(
                &mut *tx,
                "system",
                "cleanup_unreferenced_revision",
                &record.package_id,
                Some(&record.revision_id),
                "ok",
                json!({"outcome": "deleted"}),
            )
            .await?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn record_cleanup_failure_audit(
        &self,
        record: &SkillRevisionRecord,
        phase: &str,
    ) -> anyhow::Result<()> {
        let metadata = json!({"outcome": "retryable", "phase": phase});
        let metadata_json = serde_json::to_string(&metadata)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let exists: Option<i64> = sqlx::query_scalar(
                r#"SELECT 1 FROM skill_audit_log
                   WHERE operation = 'cleanup_unreferenced_revision'
                     AND package_id = ? AND revision_id = ?
                     AND result = 'error' AND metadata_json = ?
                   LIMIT 1"#,
            )
            .bind(record.package_id.as_str())
            .bind(&record.revision_id)
            .bind(&metadata_json)
            .fetch_optional(&mut *tx)
            .await?;
            if exists.is_none() {
                crate::skill_state::insert_audit(
                    &mut *tx,
                    "system",
                    "cleanup_unreferenced_revision",
                    &record.package_id,
                    Some(&record.revision_id),
                    "error",
                    metadata,
                )
                .await?;
            }
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

async fn revision_is_durably_protected(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    revision_id: &str,
) -> anyhow::Result<bool> {
    let installation: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM skill_installations WHERE active_revision_id = ? AND install_status != 'removed' LIMIT 1",
    )
    .bind(revision_id)
    .fetch_optional(&mut **tx)
    .await?;
    if installation.is_some() {
        return Ok(true);
    }
    let approval: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM skill_approvals WHERE revision_id = ? AND status = 'pending' LIMIT 1",
    )
    .bind(revision_id)
    .fetch_optional(&mut **tx)
    .await?;
    if approval.is_some() {
        return Ok(true);
    }
    let retention: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM skill_revision_retention WHERE revision_id = ? AND retain_until > ? LIMIT 1",
    )
    .bind(revision_id)
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(&mut **tx)
    .await?;
    if retention.is_some() {
        return Ok(true);
    }
    let leased: Option<i64> = sqlx::query_scalar(
        r#"SELECT 1
           FROM skill_snapshot_lease_revisions revisions
           JOIN skill_snapshot_leases leases ON leases.lease_id = revisions.lease_id
           WHERE revisions.revision_id = ? AND leases.expires_at > ? LIMIT 1"#,
    )
    .bind(revision_id)
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(&mut **tx)
    .await?;
    if leased.is_some() {
        return Ok(true);
    }
    let snapshots: Vec<String> = sqlx::query_scalar("SELECT members_json FROM skill_snapshots")
        .fetch_all(&mut **tx)
        .await?;
    for members in snapshots {
        let value: serde_json::Value = serde_json::from_str(&members)?;
        if crate::skill_recovery::parse_snapshot_members(value)?
            .into_iter()
            .any(|member| member.revision_id.as_deref() == Some(revision_id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn cleanup_expectation(record: &SkillRevisionRecord) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&json!({
        "packageId": record.package_id.as_str(),
        "revisionId": record.revision_id,
        "version": record.version,
        "contentHash": record.content_hash,
        "storagePath": record.storage_path,
        "descriptor": record.descriptor_json,
        "validation": record.validation_json,
        "status": record.status.as_str(),
    }))?)
}

fn state_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}
