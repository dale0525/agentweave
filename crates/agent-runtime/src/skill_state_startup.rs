use crate::skill_state::{SkillApprovalStatus, SkillRevisionRecord, SkillStateStore};
use crate::skill_state_rows::{
    APPROVAL_COLUMNS, REVISION_COLUMNS, approval_from_row, revision_from_row,
};
use chrono::Utc;
use serde_json::Value;

impl SkillStateStore {
    pub(crate) async fn list_all_revisions(&self) -> anyhow::Result<Vec<SkillRevisionRecord>> {
        let query = format!("SELECT {REVISION_COLUMNS} FROM skill_revisions ORDER BY revision_id");
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(revision_from_row)
            .collect()
    }

    pub(crate) async fn record_maintenance_diagnostic_once(
        &self,
        key: &str,
        revision_id: Option<&str>,
        area: &str,
        operation: &str,
        metadata: Value,
    ) -> anyhow::Result<bool> {
        let changed = sqlx::query(
            r#"INSERT INTO skill_maintenance_diagnostics
               (idempotency_key, revision_id, area, operation, outcome, metadata_json, created_at)
               VALUES (?, ?, ?, ?, 'preserved', ?, ?)
               ON CONFLICT(idempotency_key) DO NOTHING"#,
        )
        .bind(key)
        .bind(revision_id)
        .bind(area)
        .bind(operation)
        .bind(serde_json::to_string(&metadata)?)
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool())
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub(crate) async fn maintenance_diagnostic_count(&self) -> anyhow::Result<usize> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_maintenance_diagnostics")
            .fetch_one(self.pool())
            .await?;
        Ok(usize::try_from(count)?)
    }

    pub(crate) async fn reconcile_stale_pending_approvals(
        &self,
        current_generation: u64,
    ) -> anyhow::Result<usize> {
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let query = format!(
                "SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE status = 'pending' ORDER BY approval_id"
            );
            let rows = sqlx::query(&query).fetch_all(&mut *tx).await?;
            let mut resolved = 0usize;
            for row in rows {
                let approval = approval_from_row(&row)?;
                debug_assert_eq!(approval.status, SkillApprovalStatus::Pending);
                let binding: Option<String> = sqlx::query_scalar(
                    "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
                )
                .bind(&approval.approval_id)
                .fetch_optional(&mut *tx)
                .await?;
                let stale = match binding {
                    Some(binding) => {
                        let binding: Value = serde_json::from_str(&binding)?;
                        approval_binding_is_stale(
                            &mut tx,
                            &approval.revision_id,
                            &binding,
                            current_generation,
                        )
                        .await?
                    }
                    None => true,
                };
                if !stale {
                    continue;
                }
                let changed = sqlx::query(
                    r#"UPDATE skill_approvals
                       SET approved_by = 'system-recovery', status = 'rejected', resolved_at = ?
                       WHERE approval_id = ? AND status = 'pending'"#,
                )
                .bind(Utc::now().to_rfc3339())
                .bind(&approval.approval_id)
                .execute(&mut *tx)
                .await?;
                resolved += usize::try_from(changed.rows_affected())?;
            }
            Ok(resolved)
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

async fn approval_binding_is_stale(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    revision_id: &str,
    binding: &Value,
    current_generation: u64,
) -> anyhow::Result<bool> {
    let generation = binding
        .get("validationSnapshotGeneration")
        .or_else(|| binding.get("snapshotGeneration"))
        .and_then(Value::as_u64);
    if generation != Some(current_generation) {
        return Ok(true);
    }
    let query = format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
    let record = sqlx::query(&query)
        .bind(revision_id)
        .fetch_optional(&mut **tx)
        .await?
        .map(|row| revision_from_row(&row))
        .transpose()?;
    let Some(record) = record else {
        return Ok(true);
    };
    let hash = binding.get("contentHash").and_then(Value::as_str);
    let path = binding.get("revisionStoragePath").and_then(Value::as_str);
    Ok(hash != Some(record.content_hash.as_str())
        || path.is_some_and(|path| path != record.storage_path))
}
