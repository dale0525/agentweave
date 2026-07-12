use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, SkillApprovalRecord, SkillInstallationRecord, SkillRevisionExpectation,
    SkillRevisionPromotion, SkillStateStore,
};
use crate::skill_state_rows::{
    APPROVAL_COLUMNS, INSTALLATION_COLUMNS, approval_from_row, installation_from_row,
};
use anyhow::Context;
use chrono::Utc;

pub(crate) struct ExactActivationPublication<'a> {
    pub approval_id: &'a str,
    pub approver_id: &'a str,
    pub expected_binding: &'a serde_json::Value,
    pub package_id: &'a SkillPackageId,
    pub revision_id: &'a str,
    pub expectation: &'a SkillRevisionExpectation,
    pub promotion: &'a SkillRevisionPromotion,
    pub previous_installation: Option<&'a SkillInstallationRecord>,
    pub generation: u64,
    pub members: serde_json::Value,
}

impl SkillStateStore {
    pub(crate) async fn commit_exact_activation_publication(
        &self,
        input: ExactActivationPublication<'_>,
    ) -> anyhow::Result<()> {
        let ExactActivationPublication {
            approval_id,
            approver_id,
            expected_binding,
            package_id,
            revision_id,
            expectation,
            promotion,
            previous_installation,
            generation,
            members,
        } = input;
        crate::skill_state_rows::validate_uuid_v4("approval_id", approval_id)?;
        let generation =
            i64::try_from(generation).context("snapshot generation exceeds SQLite range")?;
        let members = serde_json::to_string(&members)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let approval_query = format!(
                "SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?"
            );
            let approval_row = sqlx::query(&approval_query)
                .bind(approval_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("skill approval not found")?;
            let approval = approval_from_row(&approval_row)?;
            if approval.status != crate::skill_state::SkillApprovalStatus::Pending
                || approval.requested_by == approver_id
                || approval.package_id != *package_id
                || approval.revision_id != revision_id
            {
                anyhow::bail!("skill approval conflicts with current state");
            }
            let binding_json: String = sqlx::query_scalar(
                "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
            )
            .bind(approval_id)
            .fetch_optional(&mut *tx)
            .await?
            .context("activation approval binding is missing")?;
            if serde_json::from_str::<serde_json::Value>(&binding_json)? != *expected_binding {
                anyhow::bail!("activation approval binding is stale");
            }

            let installation_query = format!(
                "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?"
            );
            let current_installation = sqlx::query(&installation_query)
                .bind(package_id.as_str())
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| installation_from_row(&row))
                .transpose()?;
            if current_installation.as_ref() != previous_installation {
                anyhow::bail!("skill installation changed during activation");
            }

            let changed = sqlx::query(
                r#"UPDATE skill_revisions SET version = ?, content_hash = ?, storage_path = ?,
                   descriptor_json = ?, validation_json = ?, lifecycle_status = 'managed'
                   WHERE revision_id = ? AND package_id = ? AND version = ? AND content_hash = ?
                     AND storage_path = ? AND descriptor_json = ? AND validation_json = ?
                     AND lifecycle_status = 'staging'"#,
            )
            .bind(&promotion.version)
            .bind(&promotion.content_hash)
            .bind(&promotion.storage_path)
            .bind(serde_json::to_string(&promotion.descriptor_json)?)
            .bind(serde_json::to_string(&promotion.validation_json)?)
            .bind(revision_id)
            .bind(package_id.as_str())
            .bind(&expectation.version)
            .bind(&expectation.content_hash)
            .bind(&expectation.storage_path)
            .bind(serde_json::to_string(&expectation.descriptor_json)?)
            .bind(serde_json::to_string(&expectation.validation_json)?)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                anyhow::bail!("skill revision changed during activation");
            }

            sqlx::query(
                r#"INSERT INTO skill_installations
                   (package_id, source_layer, active_revision_id, enabled, trust_level,
                    install_status, installed_at, updated_at)
                   VALUES (?, 'managed', ?, 1, 'approved', 'active', ?, ?)
                   ON CONFLICT(package_id) DO UPDATE SET source_layer = 'managed',
                     active_revision_id = excluded.active_revision_id, enabled = 1,
                     trust_level = 'approved', install_status = 'active',
                     updated_at = excluded.updated_at"#,
            )
            .bind(package_id.as_str())
            .bind(revision_id)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;

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
                anyhow::bail!("skill approval could not be resolved");
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
                let existing_binding: Option<String> = sqlx::query_scalar(
                    "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
                )
                .bind(&existing.approval_id)
                .fetch_optional(&mut *tx)
                .await?;
                if existing.requested_by == input.requested_by
                    && existing.permission_diff == input.permission_diff
                    && existing_binding
                        .as_deref()
                        .map(serde_json::from_str::<serde_json::Value>)
                        .transpose()?
                        == input.binding
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
            if let Some(binding) = input.binding {
                sqlx::query(
                    "INSERT INTO skill_approval_bindings (approval_id, binding_json) VALUES (?, ?)",
                )
                .bind(&approval.approval_id)
                .bind(serde_json::to_string(&binding)?)
                .execute(&mut *tx)
                .await?;
            }
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

    pub(crate) async fn activation_approval_binding(
        &self,
        approval_id: &str,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        crate::skill_state_rows::validate_uuid_v4("approval_id", approval_id)?;
        let binding: Option<String> = sqlx::query_scalar(
            "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
        )
        .bind(approval_id)
        .fetch_optional(self.pool())
        .await?;
        binding
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(Into::into)
    }
}
