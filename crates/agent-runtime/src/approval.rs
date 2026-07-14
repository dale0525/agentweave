use crate::durable_run::DurableRunStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    ReadSensitive,
    ExternalWrite,
    DestructiveWrite,
    CredentialAccess,
    CodeExecution,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovalBinding {
    pub actor_id: String,
    pub app_id: String,
    pub run_id: String,
    pub action_id: String,
    pub action_name: String,
    pub arguments_sha256: String,
    pub resource_target: String,
    pub policy_version: String,
    pub risk: ApprovalRisk,
    pub risk_summary: String,
    pub session_id: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl ApprovalBinding {
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [
            &self.actor_id,
            &self.app_id,
            &self.run_id,
            &self.action_id,
            &self.action_name,
            &self.arguments_sha256,
            &self.resource_target,
            &self.policy_version,
            &self.risk_summary,
        ] {
            anyhow::ensure!(
                !value.trim().is_empty(),
                "approval binding field is required"
            );
            anyhow::ensure!(value.len() <= 1024, "approval binding field is too long");
        }
        anyhow::ensure!(
            self.arguments_sha256.len() == 64
                && self
                    .arguments_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "approval arguments hash is invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    ApproveSession,
    Reject,
    Cancel,
}

impl ApprovalDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::ApproveOnce => "approve_once",
            Self::ApproveSession => "approve_session",
            Self::Reject => "reject",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Expired,
    Consumed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApprovalRecord {
    pub approval_id: String,
    pub binding: ApprovalBinding,
    pub status: ApprovalStatus,
    pub decision: Option<ApprovalDecision>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

impl DurableRunStore {
    pub async fn request_approval(
        &self,
        binding: ApprovalBinding,
        now: DateTime<Utc>,
    ) -> anyhow::Result<ApprovalRecord> {
        binding.validate()?;
        anyhow::ensure!(
            binding.expires_at > now,
            "approval expiry must be in the future"
        );
        let approval_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO run_approvals(approval_id, run_id, action_id, binding_json, status, decision, resolved_by, resolved_at, consumed_at, created_at) VALUES (?, ?, ?, ?, 'pending', NULL, NULL, NULL, NULL, ?)",
        )
        .bind(&approval_id)
        .bind(&binding.run_id)
        .bind(&binding.action_id)
        .bind(serde_json::to_string(&binding)?)
        .bind(now.to_rfc3339())
        .execute(self.pool())
        .await?;
        self.get_approval(&approval_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("approval was not persisted"))
    }

    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        resolver: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<ApprovalRecord> {
        anyhow::ensure!(!resolver.trim().is_empty(), "approval resolver is required");
        let current = self
            .get_approval(approval_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("approval not found"))?;
        if current.status != ApprovalStatus::Pending {
            return Ok(current);
        }
        let (status, decision) = if current.binding.expires_at <= now {
            ("expired", None)
        } else {
            let status = match decision {
                ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveSession => "approved",
                ApprovalDecision::Reject => "rejected",
                ApprovalDecision::Cancel => "cancelled",
            };
            (status, Some(decision))
        };
        sqlx::query(
            "UPDATE run_approvals SET status = ?, decision = ?, resolved_by = ?, resolved_at = ? WHERE approval_id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(decision.map(ApprovalDecision::as_str))
        .bind(resolver)
        .bind(now.to_rfc3339())
        .bind(approval_id)
        .execute(self.pool())
        .await?;
        self.get_approval(approval_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("approval disappeared"))
    }

    pub async fn consume_approval(
        &self,
        approval_id: &str,
        expected: &ApprovalBinding,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        expected.validate()?;
        let record = self
            .get_approval(approval_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("approval not found"))?;
        anyhow::ensure!(
            record.binding == *expected,
            "approval action binding changed"
        );
        anyhow::ensure!(record.binding.expires_at > now, "approval expired");
        anyhow::ensure!(
            record.status == ApprovalStatus::Approved,
            "approval is not executable"
        );
        let mut tx = self.pool().begin().await?;
        let reusable = record.decision == Some(ApprovalDecision::ApproveSession);
        let updated = if reusable {
            1
        } else {
            sqlx::query(
                "UPDATE run_approvals SET status = 'consumed', consumed_at = ? WHERE approval_id = ? AND status = 'approved'",
            )
            .bind(now.to_rfc3339())
            .bind(approval_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        };
        if updated == 1 {
            let action = sqlx::query(
                "UPDATE durable_actions SET status = 'ready', version = version + 1, updated_at = ? WHERE action_id = ? AND approval_id = ? AND status = 'waiting_approval'",
            )
            .bind(now.to_rfc3339())
            .bind(&record.binding.action_id)
            .bind(approval_id)
            .execute(&mut *tx)
            .await?;
            anyhow::ensure!(
                action.rows_affected() == 1,
                "approved action is not resumable"
            );
        }
        tx.commit().await?;
        Ok(updated == 1)
    }

    pub async fn get_approval(&self, approval_id: &str) -> anyhow::Result<Option<ApprovalRecord>> {
        let row = sqlx::query(
            "SELECT approval_id, binding_json, status, decision, resolved_by, resolved_at, consumed_at FROM run_approvals WHERE approval_id = ?",
        )
        .bind(approval_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(approval_from_row).transpose()
    }

    pub async fn expire_approvals(&self, now: DateTime<Utc>) -> anyhow::Result<u64> {
        let rows = sqlx::query(
            "SELECT approval_id, binding_json FROM run_approvals WHERE status = 'pending'",
        )
        .fetch_all(self.pool())
        .await?;
        let mut expired = 0;
        for row in rows {
            let binding: ApprovalBinding = serde_json::from_str(row.try_get("binding_json")?)?;
            if binding.expires_at <= now {
                expired += sqlx::query(
                    "UPDATE run_approvals SET status = 'expired', resolved_at = ? WHERE approval_id = ? AND status = 'pending'",
                )
                .bind(now.to_rfc3339())
                .bind(row.try_get::<String, _>("approval_id")?)
                .execute(self.pool())
                .await?
                .rows_affected();
            }
        }
        Ok(expired)
    }
}

pub fn immutable_arguments_hash(arguments: &Value) -> anyhow::Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &canonical_json(arguments),
    )?)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        _ => value.clone(),
    }
}

pub(crate) fn approval_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<ApprovalRecord> {
    let status: String = row.try_get("status")?;
    let decision: Option<String> = row.try_get("decision")?;
    Ok(ApprovalRecord {
        approval_id: row.try_get("approval_id")?,
        binding: serde_json::from_str(row.try_get("binding_json")?)?,
        status: match status.as_str() {
            "pending" => ApprovalStatus::Pending,
            "approved" => ApprovalStatus::Approved,
            "rejected" => ApprovalStatus::Rejected,
            "cancelled" => ApprovalStatus::Cancelled,
            "expired" => ApprovalStatus::Expired,
            "consumed" => ApprovalStatus::Consumed,
            _ => anyhow::bail!("invalid persisted approval status"),
        },
        decision: decision
            .map(|value| match value.as_str() {
                "approve_once" => Ok(ApprovalDecision::ApproveOnce),
                "approve_session" => Ok(ApprovalDecision::ApproveSession),
                "reject" => Ok(ApprovalDecision::Reject),
                "cancel" => Ok(ApprovalDecision::Cancel),
                _ => anyhow::bail!("invalid persisted approval decision"),
            })
            .transpose()?,
        resolved_by: row.try_get("resolved_by")?,
        resolved_at: parse_optional_time(row.try_get("resolved_at")?)?,
        consumed_at: parse_optional_time(row.try_get("consumed_at")?)?,
    })
}

fn parse_optional_time(value: Option<String>) -> anyhow::Result<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(Into::into)
        })
        .transpose()
}
