use crate::approval::immutable_arguments_hash;
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration as StdDuration;
use uuid::Uuid;

const RUN_SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    WaitingApproval,
    Retrying,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
    Uncertain,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Uncertain => "uncertain",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "retrying" => Ok(Self::Retrying),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "uncertain" => Ok(Self::Uncertain),
            _ => anyhow::bail!("invalid persisted run status"),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired | Self::Uncertain
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunScope {
    pub app_id: String,
    pub agent_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DurableRun {
    pub run_id: String,
    pub scope: RunScope,
    pub objective: String,
    pub status: RunStatus,
    pub checkpoint: Value,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Queued,
    Running,
    WaitingApproval,
    Retrying,
    Succeeded,
    Failed,
    Cancelled,
    Uncertain,
}

impl StepStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DurableStep {
    pub step_id: String,
    pub run_id: String,
    pub sequence: i64,
    pub kind: String,
    pub status: StepStatus,
    pub input: Value,
    pub attempt_count: i64,
    pub version: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Pending,
    WaitingApproval,
    Ready,
    Executing,
    Succeeded,
    Failed,
    Cancelled,
    Uncertain,
}

impl ActionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::WaitingApproval => "waiting_approval",
            Self::Ready => "ready",
            Self::Executing => "executing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DurableAction {
    pub action_id: String,
    pub run_id: String,
    pub step_id: String,
    pub action_name: String,
    pub arguments: Value,
    pub arguments_sha256: String,
    pub resource_target: String,
    pub idempotency_key: String,
    pub status: ActionStatus,
    pub approval_id: Option<String>,
    pub result: Option<Value>,
    pub last_error: Option<String>,
    pub replayed: bool,
    pub version: i64,
}

pub struct QueueActionRequest<'a> {
    pub run_id: &'a str,
    pub step_id: &'a str,
    pub action_name: &'a str,
    pub arguments: Value,
    pub resource_target: &'a str,
    pub idempotency_key: &'a str,
    pub approval_required: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Succeeded,
    Failed,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    Pending,
    Delivering,
    Delivered,
    Failed,
    Uncertain,
    Cancelled,
}

impl OutboxStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivering => "delivering",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct OutboxRecord {
    pub outbox_id: String,
    pub action_id: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub status: OutboxStatus,
    pub attempt_count: i64,
    pub delivery_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct DurableRunStore {
    pool: SqlitePool,
}

impl DurableRunStore {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    pub async fn from_storage(storage: &Storage) -> anyhow::Result<Self> {
        let store = Self {
            pool: storage.pool().clone(),
        };
        store.migrate().await?;
        Ok(store)
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS run_schema(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .execute(&mut *tx)
        .await?;
        let future: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM run_schema WHERE version > ?")
                .bind(RUN_SCHEMA_VERSION)
                .fetch_one(&mut *tx)
                .await?;
        anyhow::ensure!(
            future.is_none(),
            "durable run schema is newer than this runtime"
        );
        for statement in schema_statements() {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT OR IGNORE INTO run_schema(version, applied_at) VALUES (?, ?)")
            .bind(RUN_SCHEMA_VERSION)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn create_run(
        &self,
        scope: RunScope,
        objective: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<DurableRun> {
        validate_scope(&scope)?;
        anyhow::ensure!(!objective.trim().is_empty(), "run objective is required");
        anyhow::ensure!(objective.len() <= 16 * 1024, "run objective is too long");
        let run = DurableRun {
            run_id: Uuid::new_v4().to_string(),
            scope,
            objective: objective.to_string(),
            status: RunStatus::Queued,
            checkpoint: Value::Null,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(
            "INSERT INTO durable_runs(run_id, app_id, agent_id, tenant_id, user_id, session_id, objective, status, checkpoint_json, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.run_id)
        .bind(&run.scope.app_id)
        .bind(&run.scope.agent_id)
        .bind(&run.scope.tenant_id)
        .bind(&run.scope.user_id)
        .bind(&run.scope.session_id)
        .bind(&run.objective)
        .bind(run.status.as_str())
        .bind(serde_json::to_string(&run.checkpoint)?)
        .bind(run.version)
        .bind(run.created_at.to_rfc3339())
        .bind(run.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(run)
    }

    pub async fn transition_run(
        &self,
        run_id: &str,
        expected_version: i64,
        next: RunStatus,
        checkpoint: Value,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let current: Option<String> =
            sqlx::query_scalar("SELECT status FROM durable_runs WHERE run_id = ? AND version = ?")
                .bind(run_id)
                .bind(expected_version)
                .fetch_optional(&self.pool)
                .await?;
        let Some(current) = current else {
            return Ok(false);
        };
        let current = RunStatus::parse(&current)?;
        anyhow::ensure!(
            valid_run_transition(current, next),
            "invalid durable run transition"
        );
        anyhow::ensure!(
            serde_json::to_vec(&checkpoint)?.len() <= 256 * 1024,
            "run checkpoint exceeds limit"
        );
        let updated = sqlx::query(
            "UPDATE durable_runs SET status = ?, checkpoint_json = ?, version = version + 1, updated_at = ? WHERE run_id = ? AND version = ? AND status = ?",
        )
        .bind(next.as_str())
        .bind(serde_json::to_string(&checkpoint)?)
        .bind(now.to_rfc3339())
        .bind(run_id)
        .bind(expected_version)
        .bind(current.as_str())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn add_step(
        &self,
        run_id: &str,
        sequence: i64,
        kind: &str,
        input: Value,
        now: DateTime<Utc>,
    ) -> anyhow::Result<DurableStep> {
        anyhow::ensure!(sequence >= 0, "step sequence cannot be negative");
        anyhow::ensure!(!kind.trim().is_empty(), "step kind is required");
        anyhow::ensure!(
            serde_json::to_vec(&input)?.len() <= 256 * 1024,
            "step input exceeds limit"
        );
        let step = DurableStep {
            step_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            sequence,
            kind: kind.to_string(),
            status: StepStatus::Queued,
            input,
            attempt_count: 0,
            version: 1,
        };
        sqlx::query(
            "INSERT INTO run_steps(step_id, run_id, sequence, kind, status, input_json, output_json, error_json, attempt_count, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, 0, 1, ?, ?)",
        )
        .bind(&step.step_id)
        .bind(&step.run_id)
        .bind(step.sequence)
        .bind(&step.kind)
        .bind(step.status.as_str())
        .bind(serde_json::to_string(&step.input)?)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(step)
    }

    pub async fn queue_action(
        &self,
        request: QueueActionRequest<'_>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<DurableAction> {
        let QueueActionRequest {
            run_id,
            step_id,
            action_name,
            arguments,
            resource_target,
            idempotency_key,
            approval_required,
        } = request;
        anyhow::ensure!(!action_name.trim().is_empty(), "action name is required");
        anyhow::ensure!(
            !resource_target.trim().is_empty(),
            "action target is required"
        );
        anyhow::ensure!(
            !idempotency_key.trim().is_empty(),
            "action idempotency key is required"
        );
        let arguments_sha256 = immutable_arguments_hash(&arguments)?;
        if let Some(mut existing) = self.action_by_idempotency(run_id, idempotency_key).await? {
            anyhow::ensure!(
                existing.arguments_sha256 == arguments_sha256
                    && existing.action_name == action_name
                    && existing.resource_target == resource_target,
                "action idempotency conflict"
            );
            existing.replayed = true;
            return Ok(existing);
        }
        let action = DurableAction {
            action_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            action_name: action_name.to_string(),
            arguments,
            arguments_sha256,
            resource_target: resource_target.to_string(),
            idempotency_key: idempotency_key.to_string(),
            status: if approval_required {
                ActionStatus::WaitingApproval
            } else {
                ActionStatus::Ready
            },
            approval_id: None,
            result: None,
            last_error: None,
            replayed: false,
            version: 1,
        };
        sqlx::query(
            "INSERT INTO durable_actions(action_id, run_id, step_id, action_name, arguments_json, arguments_sha256, resource_target, idempotency_key, status, approval_id, result_json, last_error, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, 1, ?, ?)",
        )
        .bind(&action.action_id)
        .bind(&action.run_id)
        .bind(&action.step_id)
        .bind(&action.action_name)
        .bind(serde_json::to_string(&action.arguments)?)
        .bind(&action.arguments_sha256)
        .bind(&action.resource_target)
        .bind(&action.idempotency_key)
        .bind(action.status.as_str())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(action)
    }

    pub async fn bind_action_approval(
        &self,
        action_id: &str,
        approval_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let updated = sqlx::query(
            "UPDATE durable_actions SET approval_id = ?, version = version + 1, updated_at = ? WHERE action_id = ? AND status = 'waiting_approval' AND approval_id IS NULL",
        )
        .bind(approval_id)
        .bind(now.to_rfc3339())
        .bind(action_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn begin_action(
        &self,
        action_id: &str,
        expected_version: i64,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let updated = sqlx::query(
            "UPDATE durable_actions SET status = 'executing', version = version + 1, updated_at = ? WHERE action_id = ? AND version = ? AND status = 'ready'",
        )
        .bind(now.to_rfc3339())
        .bind(action_id)
        .bind(expected_version)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn complete_action(
        &self,
        action_id: &str,
        outcome: ActionOutcome,
        result: Value,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let status = match outcome {
            ActionOutcome::Succeeded => ActionStatus::Succeeded,
            ActionOutcome::Failed => ActionStatus::Failed,
            ActionOutcome::Cancelled => ActionStatus::Cancelled,
            ActionOutcome::Uncertain => ActionStatus::Uncertain,
        };
        let updated = sqlx::query(
            "UPDATE durable_actions SET status = ?, result_json = ?, last_error = ?, version = version + 1, updated_at = ? WHERE action_id = ? AND status = 'executing'",
        )
        .bind(status.as_str())
        .bind(serde_json::to_string(&result)?)
        .bind(error)
        .bind(now.to_rfc3339())
        .bind(action_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn enqueue_outbox(
        &self,
        action_id: &str,
        idempotency_key: &str,
        payload: Value,
        now: DateTime<Utc>,
    ) -> anyhow::Result<OutboxRecord> {
        if let Some(existing) = self.outbox_by_key(idempotency_key).await? {
            anyhow::ensure!(
                existing.action_id == action_id && existing.payload == payload,
                "outbox idempotency conflict"
            );
            return Ok(existing);
        }
        let record = OutboxRecord {
            outbox_id: Uuid::new_v4().to_string(),
            action_id: action_id.to_string(),
            idempotency_key: idempotency_key.to_string(),
            payload,
            status: OutboxStatus::Pending,
            attempt_count: 0,
            delivery_id: None,
            last_error: None,
        };
        sqlx::query(
            "INSERT INTO run_outbox(outbox_id, action_id, idempotency_key, payload_json, status, attempt_count, delivery_id, last_error, created_at, updated_at) VALUES (?, ?, ?, ?, 'pending', 0, NULL, NULL, ?, ?)",
        )
        .bind(&record.outbox_id)
        .bind(&record.action_id)
        .bind(&record.idempotency_key)
        .bind(serde_json::to_string(&record.payload)?)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(record)
    }

    pub async fn claim_outbox(&self, outbox_id: &str, now: DateTime<Utc>) -> anyhow::Result<bool> {
        let updated = sqlx::query(
            "UPDATE run_outbox SET status = 'delivering', attempt_count = attempt_count + 1, updated_at = ? WHERE outbox_id = ? AND status IN ('pending', 'failed')",
        )
        .bind(now.to_rfc3339())
        .bind(outbox_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn finish_outbox(
        &self,
        outbox_id: &str,
        status: OutboxStatus,
        delivery_id: Option<&str>,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            matches!(
                status,
                OutboxStatus::Delivered
                    | OutboxStatus::Failed
                    | OutboxStatus::Uncertain
                    | OutboxStatus::Cancelled
            ),
            "outbox terminal status is invalid"
        );
        let updated = sqlx::query(
            "UPDATE run_outbox SET status = ?, delivery_id = ?, last_error = ?, updated_at = ? WHERE outbox_id = ? AND status = 'delivering'",
        )
        .bind(status.as_str())
        .bind(delivery_id)
        .bind(error)
        .bind(now.to_rfc3339())
        .bind(outbox_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn recoverable_runs(&self) -> anyhow::Result<Vec<DurableRun>> {
        let rows = sqlx::query(
            "SELECT run_id, app_id, agent_id, tenant_id, user_id, session_id, objective, status, checkpoint_json, version, created_at, updated_at FROM durable_runs WHERE status IN ('queued', 'running', 'waiting_approval', 'retrying') ORDER BY created_at, run_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(run_from_row).collect()
    }

    pub async fn get_run(&self, run_id: &str) -> anyhow::Result<Option<DurableRun>> {
        let row = sqlx::query(
            "SELECT run_id, app_id, agent_id, tenant_id, user_id, session_id, objective, status, checkpoint_json, version, created_at, updated_at FROM durable_runs WHERE run_id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(run_from_row).transpose()
    }

    pub async fn find_scoped_action_by_idempotency(
        &self,
        scope: &RunScope,
        action_name: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<DurableAction>> {
        validate_scope(scope)?;
        let row = sqlx::query(
            "SELECT a.action_id, a.run_id, a.step_id, a.action_name, a.arguments_json, a.arguments_sha256, a.resource_target, a.idempotency_key, a.status, a.approval_id, a.result_json, a.last_error, a.version FROM durable_actions a JOIN durable_runs r ON r.run_id = a.run_id WHERE r.app_id = ? AND r.agent_id = ? AND r.tenant_id = ? AND r.user_id = ? AND ((r.session_id IS NULL AND ? IS NULL) OR r.session_id = ?) AND a.action_name = ? AND a.idempotency_key = ? ORDER BY r.created_at DESC LIMIT 1",
        )
        .bind(&scope.app_id)
        .bind(&scope.agent_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(&scope.session_id)
        .bind(&scope.session_id)
        .bind(action_name)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(action_from_row).transpose()
    }

    pub async fn list_approvals_for_scope(
        &self,
        app_id: &str,
        tenant_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Vec<crate::approval::ApprovalRecord>> {
        let rows = sqlx::query(
            "SELECT p.approval_id, p.binding_json, p.status, p.decision, p.resolved_by, p.resolved_at, p.consumed_at FROM run_approvals p JOIN durable_runs r ON r.run_id = p.run_id WHERE r.app_id = ? AND r.tenant_id = ? AND r.user_id = ? ORDER BY p.created_at DESC, p.approval_id",
        )
        .bind(app_id)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(crate::approval::approval_from_row)
            .collect()
    }

    pub async fn cancel_waiting_action(
        &self,
        action_id: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            !reason.trim().is_empty(),
            "action cancellation reason is required"
        );
        let updated = sqlx::query(
            "UPDATE durable_actions SET status = 'cancelled', last_error = ?, version = version + 1, updated_at = ? WHERE action_id = ? AND status IN ('pending', 'waiting_approval', 'ready')",
        )
        .bind(reason)
        .bind(now.to_rfc3339())
        .bind(action_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    async fn action_by_idempotency(
        &self,
        run_id: &str,
        key: &str,
    ) -> anyhow::Result<Option<DurableAction>> {
        let row = sqlx::query(
            "SELECT action_id, run_id, step_id, action_name, arguments_json, arguments_sha256, resource_target, idempotency_key, status, approval_id, result_json, last_error, version FROM durable_actions WHERE run_id = ? AND idempotency_key = ?",
        )
        .bind(run_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(action_from_row).transpose()
    }

    pub async fn get_action(&self, action_id: &str) -> anyhow::Result<Option<DurableAction>> {
        let row = sqlx::query(
            "SELECT action_id, run_id, step_id, action_name, arguments_json, arguments_sha256, resource_target, idempotency_key, status, approval_id, result_json, last_error, version FROM durable_actions WHERE action_id = ?",
        )
        .bind(action_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(action_from_row).transpose()
    }

    async fn outbox_by_key(&self, key: &str) -> anyhow::Result<Option<OutboxRecord>> {
        let row = sqlx::query(
            "SELECT outbox_id, action_id, idempotency_key, payload_json, status, attempt_count, delivery_id, last_error FROM run_outbox WHERE idempotency_key = ?",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(outbox_from_row).transpose()
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn schema_statements() -> [&'static str; 7] {
    [
        r#"CREATE TABLE IF NOT EXISTS durable_runs (run_id TEXT PRIMARY KEY, app_id TEXT NOT NULL, agent_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, session_id TEXT, objective TEXT NOT NULL, status TEXT NOT NULL, checkpoint_json TEXT NOT NULL, version INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)"#,
        r#"CREATE TABLE IF NOT EXISTS run_steps (step_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, sequence INTEGER NOT NULL, kind TEXT NOT NULL, status TEXT NOT NULL, input_json TEXT NOT NULL, output_json TEXT, error_json TEXT, attempt_count INTEGER NOT NULL, version INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, FOREIGN KEY(run_id) REFERENCES durable_runs(run_id), UNIQUE(run_id, sequence))"#,
        r#"CREATE TABLE IF NOT EXISTS durable_actions (action_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, step_id TEXT NOT NULL, action_name TEXT NOT NULL, arguments_json TEXT NOT NULL, arguments_sha256 TEXT NOT NULL, resource_target TEXT NOT NULL, idempotency_key TEXT NOT NULL, status TEXT NOT NULL, approval_id TEXT, result_json TEXT, last_error TEXT, version INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, FOREIGN KEY(run_id) REFERENCES durable_runs(run_id), FOREIGN KEY(step_id) REFERENCES run_steps(step_id), UNIQUE(run_id, idempotency_key))"#,
        r#"CREATE TABLE IF NOT EXISTS action_attempts (attempt_id TEXT PRIMARY KEY, action_id TEXT NOT NULL, attempt_number INTEGER NOT NULL, status TEXT NOT NULL, error_class TEXT, started_at TEXT NOT NULL, finished_at TEXT, FOREIGN KEY(action_id) REFERENCES durable_actions(action_id), UNIQUE(action_id, attempt_number))"#,
        r#"CREATE TABLE IF NOT EXISTS run_approvals (approval_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, action_id TEXT NOT NULL, binding_json TEXT NOT NULL, status TEXT NOT NULL, decision TEXT, resolved_by TEXT, resolved_at TEXT, consumed_at TEXT, created_at TEXT NOT NULL, FOREIGN KEY(run_id) REFERENCES durable_runs(run_id), FOREIGN KEY(action_id) REFERENCES durable_actions(action_id))"#,
        r#"CREATE TABLE IF NOT EXISTS run_outbox (outbox_id TEXT PRIMARY KEY, action_id TEXT NOT NULL, idempotency_key TEXT NOT NULL UNIQUE, payload_json TEXT NOT NULL, status TEXT NOT NULL, attempt_count INTEGER NOT NULL, delivery_id TEXT, last_error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, FOREIGN KEY(action_id) REFERENCES durable_actions(action_id))"#,
        r#"CREATE INDEX IF NOT EXISTS durable_runs_recovery_idx ON durable_runs(status, updated_at)"#,
    ]
}

fn validate_scope(scope: &RunScope) -> anyhow::Result<()> {
    for value in [
        &scope.app_id,
        &scope.agent_id,
        &scope.tenant_id,
        &scope.user_id,
    ] {
        anyhow::ensure!(!value.trim().is_empty(), "run scope is required");
        anyhow::ensure!(value.len() <= 255, "run scope value is too long");
    }
    Ok(())
}

fn valid_run_transition(current: RunStatus, next: RunStatus) -> bool {
    match current {
        RunStatus::Queued => matches!(
            next,
            RunStatus::Running | RunStatus::Cancelled | RunStatus::Expired
        ),
        RunStatus::Running => matches!(
            next,
            RunStatus::WaitingApproval
                | RunStatus::Retrying
                | RunStatus::Succeeded
                | RunStatus::Failed
                | RunStatus::Cancelled
                | RunStatus::Uncertain
        ),
        RunStatus::WaitingApproval => matches!(
            next,
            RunStatus::Running | RunStatus::Cancelled | RunStatus::Expired
        ),
        RunStatus::Retrying => matches!(
            next,
            RunStatus::Running | RunStatus::Failed | RunStatus::Cancelled | RunStatus::Uncertain
        ),
        status if status.is_terminal() => false,
        _ => false,
    }
}

fn action_from_row(row: SqliteRow) -> anyhow::Result<DurableAction> {
    let status: String = row.try_get("status")?;
    Ok(DurableAction {
        action_id: row.try_get("action_id")?,
        run_id: row.try_get("run_id")?,
        step_id: row.try_get("step_id")?,
        action_name: row.try_get("action_name")?,
        arguments: serde_json::from_str(row.try_get("arguments_json")?)?,
        arguments_sha256: row.try_get("arguments_sha256")?,
        resource_target: row.try_get("resource_target")?,
        idempotency_key: row.try_get("idempotency_key")?,
        status: match status.as_str() {
            "pending" => ActionStatus::Pending,
            "waiting_approval" => ActionStatus::WaitingApproval,
            "ready" => ActionStatus::Ready,
            "executing" => ActionStatus::Executing,
            "succeeded" => ActionStatus::Succeeded,
            "failed" => ActionStatus::Failed,
            "cancelled" => ActionStatus::Cancelled,
            "uncertain" => ActionStatus::Uncertain,
            _ => anyhow::bail!("invalid persisted action status"),
        },
        approval_id: row.try_get("approval_id")?,
        result: row
            .try_get::<Option<String>, _>("result_json")?
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        last_error: row.try_get("last_error")?,
        replayed: false,
        version: row.try_get("version")?,
    })
}

fn run_from_row(row: SqliteRow) -> anyhow::Result<DurableRun> {
    let status: String = row.try_get("status")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(DurableRun {
        run_id: row.try_get("run_id")?,
        scope: RunScope {
            app_id: row.try_get("app_id")?,
            agent_id: row.try_get("agent_id")?,
            tenant_id: row.try_get("tenant_id")?,
            user_id: row.try_get("user_id")?,
            session_id: row.try_get("session_id")?,
        },
        objective: row.try_get("objective")?,
        status: RunStatus::parse(&status)?,
        checkpoint: serde_json::from_str(row.try_get("checkpoint_json")?)?,
        version: row.try_get("version")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
    })
}

fn outbox_from_row(row: SqliteRow) -> anyhow::Result<OutboxRecord> {
    let status: String = row.try_get("status")?;
    Ok(OutboxRecord {
        outbox_id: row.try_get("outbox_id")?,
        action_id: row.try_get("action_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        payload: serde_json::from_str(row.try_get("payload_json")?)?,
        status: match status.as_str() {
            "pending" => OutboxStatus::Pending,
            "delivering" => OutboxStatus::Delivering,
            "delivered" => OutboxStatus::Delivered,
            "failed" => OutboxStatus::Failed,
            "uncertain" => OutboxStatus::Uncertain,
            "cancelled" => OutboxStatus::Cancelled,
            _ => anyhow::bail!("invalid persisted outbox status"),
        },
        attempt_count: row.try_get("attempt_count")?,
        delivery_id: row.try_get("delivery_id")?,
        last_error: row.try_get("last_error")?,
    })
}

#[cfg(test)]
#[path = "durable_run_tests.rs"]
mod tests;
