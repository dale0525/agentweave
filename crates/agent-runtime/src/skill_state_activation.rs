use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, SkillApprovalRecord, SkillInstallationRecord, SkillStateStore,
};
use crate::skill_state_rows::{APPROVAL_COLUMNS, approval_from_row};
use anyhow::Context;
use chrono::Utc;

impl SkillStateStore {
    pub(crate) async fn commit_activation_publication(
        &self,
        approval_id: &str,
        approver_id: &str,
        package_id: &SkillPackageId,
        revision_id: &str,
        generation: u64,
        members: serde_json::Value,
    ) -> anyhow::Result<()> {
        crate::skill_state_rows::validate_uuid_v4("approval_id", approval_id)?;
        let generation =
            i64::try_from(generation).context("snapshot generation exceeds SQLite range")?;
        let members = serde_json::to_string(&members)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let resolved = sqlx::query(
                r#"UPDATE skill_approvals SET approved_by = ?, status = 'approved', resolved_at = ?
                   WHERE approval_id = ? AND status = 'pending' AND requested_by != ?"#,
            )
            .bind(approver_id)
            .bind(&now)
            .bind(approval_id)
            .bind(approver_id)
            .execute(&mut *tx)
            .await?;
            if resolved.rows_affected() != 1 {
                anyhow::bail!("skill approval could not be resolved: {approval_id}");
            }
            sqlx::query(
                r#"INSERT INTO skill_snapshots
                   (generation, status, members_json, created_at, activated_at)
                   VALUES (?, 'candidate', ?, ?, NULL)"#,
            )
            .bind(generation)
            .bind(members)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'active' AND generation != ?",
            )
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            let activated = sqlx::query(
                "UPDATE skill_snapshots SET status = 'active', activated_at = ? WHERE generation = ?",
            )
            .bind(&now)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            if activated.rows_affected() != 1 {
                anyhow::bail!("snapshot publication state could not be activated");
            }
            super::skill_state::insert_audit(
                &mut *tx,
                approver_id,
                "skill_snapshot_published",
                package_id,
                Some(revision_id),
                "ok",
                serde_json::json!({"generation": generation}),
            )
            .await?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn create_activation_approval_unique(
        &self,
        input: NewSkillApproval,
    ) -> anyhow::Result<(SkillApprovalRecord, bool)> {
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let select = format!(
                "SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE revision_id = ? AND operation = ? AND status = 'pending' ORDER BY created_at LIMIT 1"
            );
            if let Some(row) = sqlx::query(&select)
                .bind(&input.revision_id)
                .bind(&input.operation)
                .fetch_optional(&mut *tx)
                .await?
            {
                let existing = approval_from_row(&row)?;
                if existing.requested_by == input.requested_by
                    && existing.permission_diff == input.permission_diff
                {
                    return Ok((existing, false));
                }
                sqlx::query(
                    "UPDATE skill_approvals SET approved_by = 'system-stale', status = 'rejected', resolved_at = ? WHERE approval_id = ? AND status = 'pending'",
                )
                .bind(Utc::now().to_rfc3339())
                .bind(&existing.approval_id)
                .execute(&mut *tx)
                .await?;
            }
            let approval_id = uuid::Uuid::new_v4().to_string();
            let created_at = Utc::now();
            let permission_diff = serde_json::to_string(&input.permission_diff)?;
            let insert = format!(
                r#"INSERT INTO skill_approvals
                   (approval_id, package_id, revision_id, operation, requested_by, approved_by,
                    status, permission_diff, created_at, resolved_at)
                   VALUES (?, ?, ?, ?, ?, NULL, 'pending', ?, ?, NULL)
                   RETURNING {APPROVAL_COLUMNS}"#
            );
            let row = sqlx::query(&insert)
                .bind(&approval_id)
                .bind(input.package_id.as_str())
                .bind(&input.revision_id)
                .bind(&input.operation)
                .bind(&input.requested_by)
                .bind(permission_diff)
                .bind(created_at.to_rfc3339())
                .fetch_one(&mut *tx)
                .await?;
            let approval = approval_from_row(&row)?;
            super::skill_state::insert_audit(
                &mut *tx,
                &input.requested_by,
                "skill_approval_required",
                &input.package_id,
                Some(&input.revision_id),
                "ok",
                serde_json::json!({"approvalId": approval.approval_id}),
            )
            .await?;
            Ok((approval, true))
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn switch_revision_for_publication(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query(
            r#"INSERT INTO skill_installations
               (package_id, source_layer, active_revision_id, enabled, trust_level,
                install_status, installed_at, updated_at)
               SELECT package_id, 'managed', revision_id, 1, 'approved', 'active', ?, ?
               FROM skill_revisions
               WHERE revision_id = ? AND package_id = ? AND lifecycle_status = 'managed'
               ON CONFLICT(package_id) DO UPDATE SET
                 source_layer = 'managed', active_revision_id = excluded.active_revision_id,
                 enabled = 1, trust_level = 'approved', install_status = 'active',
                 updated_at = excluded.updated_at"#,
        )
        .bind(&now)
        .bind(&now)
        .bind(revision_id)
        .bind(package_id.as_str())
        .execute(self.pool())
        .await?;
        if changed.rows_affected() == 0 {
            anyhow::bail!("managed revision cannot be activated");
        }
        Ok(())
    }

    pub(crate) async fn restore_installation_after_failed_publication(
        &self,
        package_id: &SkillPackageId,
        failed_revision_id: &str,
        previous: Option<&SkillInstallationRecord>,
    ) -> anyhow::Result<()> {
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let current: Option<String> = sqlx::query_scalar(
                "SELECT active_revision_id FROM skill_installations WHERE package_id = ?",
            )
            .bind(package_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
            if current.as_deref() != Some(failed_revision_id) {
                anyhow::bail!("installation changed during publication compensation");
            }
            match previous {
                None => {
                    sqlx::query(
                        "DELETE FROM skill_installations WHERE package_id = ? AND active_revision_id = ?",
                    )
                    .bind(package_id.as_str())
                    .bind(failed_revision_id)
                    .execute(&mut *tx)
                    .await?;
                }
                Some(previous) => {
                    sqlx::query(
                        r#"UPDATE skill_installations SET source_layer = ?, active_revision_id = ?,
                           enabled = ?, trust_level = ?, install_status = ?, installed_at = ?,
                           updated_at = ? WHERE package_id = ? AND active_revision_id = ?"#,
                    )
                    .bind(previous.source_layer.as_str())
                    .bind(&previous.active_revision_id)
                    .bind(previous.enabled)
                    .bind(&previous.trust_level)
                    .bind(previous.status.as_str())
                    .bind(previous.installed_at.to_rfc3339())
                    .bind(previous.updated_at.to_rfc3339())
                    .bind(package_id.as_str())
                    .bind(failed_revision_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}
